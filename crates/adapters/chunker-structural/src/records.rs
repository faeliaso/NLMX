//! Chunks for table-like files (CSV): consecutive records grouped by token budget, each chunk
//! carrying the file name, the column names and its row range so it stands on its own for
//! embeddings and retrieval.
//!
//! Pure and deterministic. The chunks are produced lazily from an iterator of record blocks, so
//! memory is proportional to one chunk (plus one chunk of lookahead), not to the file.
//!
//! Records are atomic, so there is no overlap between chunks: `ChunkPolicy::overlap_tokens` is
//! ignored here. A record larger than the budget is the one exception: it is split by fields
//! into parts that each repeat the preamble.

use std::collections::VecDeque;

use nlmx_application::ports::TokenCounter;
use nlmx_domain::{
    document_type::DocumentType,
    ingestion::ChunkPolicy,
    parsed::{ChunkContext, ContentBlock, ContentKind, DatasetMetadata, DocumentChunk},
    source::SourceLocation,
};

use crate::{sha256_hex, split::longest_fit};

/// Tokens counted for the blank line between two records.
const SEPARATOR_TOKENS: u32 = 1;
/// The columns list may take this fraction of `max_tokens` (1/4) in the preamble.
const COLUMNS_BUDGET_DIVISOR: u32 = 4;
/// Widest part counter assumed when budgeting the title of a split record.
const MAX_PARTS_PLACEHOLDER: u32 = 9999;

/// Groups the record blocks of a table-like file into chunks.
#[derive(Debug, Default, Clone, Copy)]
pub struct RecordChunker;

impl RecordChunker {
    /// Bump when the output for the same input changes.
    pub const VERSION: u32 = 1;

    pub fn version(&self) -> u32 {
        Self::VERSION
    }

    /// Chunks `records` (in file order) lazily. Each chunk's text is
    /// `Arquivo: …\nColunas: …\nLinhas a–b\n\n` followed by the records' texts separated by a
    /// blank line; its location is `SourceLocation::Csv` over the rows it holds.
    pub fn chunk<'a, I>(
        &self,
        context: &ChunkContext,
        dataset: &DatasetMetadata,
        records: I,
        policy: &ChunkPolicy,
        tokens: &'a dyn TokenCounter,
    ) -> RecordChunks<'a, I::IntoIter>
    where
        I: IntoIterator<Item = ContentBlock>,
    {
        RecordChunks::new(
            Target::Csv,
            0,
            context,
            dataset,
            records.into_iter(),
            policy,
            tokens,
        )
    }

    /// Like [`RecordChunker::chunk`] for one worksheet: the preamble also names the sheet, the
    /// location is `SourceLocation::Xlsx` and the section path is the sheet name. Chunk
    /// indexes start at `first_index`, so the sheets of a workbook number consecutively.
    #[allow(clippy::too_many_arguments)]
    pub fn chunk_sheet<'a, I>(
        &self,
        context: &ChunkContext,
        sheet_index: u32,
        sheet_name: &str,
        first_index: u32,
        dataset: &DatasetMetadata,
        records: I,
        policy: &ChunkPolicy,
        tokens: &'a dyn TokenCounter,
    ) -> RecordChunks<'a, I::IntoIter>
    where
        I: IntoIterator<Item = ContentBlock>,
    {
        let target = Target::Sheet {
            index: sheet_index,
            name: sheet_name.to_string(),
        };
        RecordChunks::new(
            target,
            first_index,
            context,
            dataset,
            records.into_iter(),
            policy,
            tokens,
        )
    }
}

/// What kind of table the records come from.
#[derive(Debug, Clone)]
enum Target {
    Csv,
    Sheet { index: u32, name: String },
}

/// A record, or one part of a record too large for a chunk.
#[derive(Debug)]
struct Piece {
    row_start: u32,
    row_end: u32,
    text: String,
    tokens: u32,
}

#[derive(Debug, Default)]
struct Group {
    pieces: Vec<Piece>,
    /// Tokens of the pieces and the separators between them (not the preamble).
    tokens: u32,
}

impl Group {
    fn push(&mut self, piece: Piece) {
        if !self.pieces.is_empty() {
            self.tokens += SEPARATOR_TOKENS;
        }
        self.tokens += piece.tokens;
        self.pieces.push(piece);
    }

    fn merge(&mut self, other: Group) {
        if !self.pieces.is_empty() && !other.pieces.is_empty() {
            self.tokens += SEPARATOR_TOKENS;
        }
        self.tokens += other.tokens;
        self.pieces.extend(other.pieces);
    }
}

/// The lazy chunk stream returned by [`RecordChunker::chunk`].
pub struct RecordChunks<'a, I> {
    target: Target,
    records: I,
    tokens: &'a dyn TokenCounter,
    file_name: String,
    context: ChunkContext,
    columns: Vec<String>,
    columns_line: String,
    /// Tokens of the preamble, counted with the widest row numbers.
    overhead: u32,
    /// Budget of a group's body: target and max minus the preamble.
    body_target: u32,
    body_max: u32,
    min_tokens: u32,
    /// Parts of a split record, waiting.
    queue: VecDeque<Piece>,
    /// A piece that did not fit the group being built.
    pending: Option<Piece>,
    /// The next group, read ahead to know whether the last one is too small.
    held: Option<(Group, bool)>,
    next_index: u32,
    /// Row number for blocks that have no CSV location.
    fallback_row: u32,
}

impl<'a, I: Iterator<Item = ContentBlock>> RecordChunks<'a, I> {
    fn new(
        target: Target,
        first_index: u32,
        context: &ChunkContext,
        dataset: &DatasetMetadata,
        records: I,
        policy: &ChunkPolicy,
        tokens: &'a dyn TokenCounter,
    ) -> Self {
        let columns = dataset.column_names();
        let columns_line =
            columns_line(&columns, policy.max_tokens / COLUMNS_BUDGET_DIVISOR, tokens);
        let mut chunks = Self {
            target,
            records,
            tokens,
            file_name: context
                .file_name
                .clone()
                .or_else(|| context.document_title.clone())
                .unwrap_or_else(|| "arquivo".to_string()),
            context: context.clone(),
            columns,
            columns_line,
            overhead: 0,
            body_target: 1,
            body_max: 1,
            min_tokens: policy.min_tokens,
            queue: VecDeque::new(),
            pending: None,
            held: None,
            next_index: first_index,
            fallback_row: 0,
        };
        chunks.overhead = tokens.count(&chunks.preamble(u32::MAX, u32::MAX));
        chunks.body_target = policy.target_tokens.saturating_sub(chunks.overhead).max(1);
        chunks.body_max = policy
            .max_tokens
            .saturating_sub(chunks.overhead)
            .max(chunks.body_target);
        chunks
    }

    fn preamble(&self, first: u32, last: u32) -> String {
        let rows = if first == last {
            format!("Linha {first}")
        } else {
            format!("Linhas {first}–{last}")
        };
        let mut text = format!("Arquivo: {}\n", self.file_name);
        if let Target::Sheet { name, .. } = &self.target {
            text.push_str(&format!("Planilha: {name}\n"));
        }
        if !self.columns_line.is_empty() {
            text.push_str(&format!("Colunas: {}\n", self.columns_line));
        }
        text.push_str(&rows);
        text.push_str("\n\n");
        text
    }

    /// The next record (or part of one), pulling from the source only when needed.
    fn take_piece(&mut self) -> Option<Piece> {
        if let Some(piece) = self.pending.take() {
            return Some(piece);
        }
        if let Some(piece) = self.queue.pop_front() {
            return Some(piece);
        }
        let block = self.records.next()?;
        let (row_start, row_end) = match block.location {
            SourceLocation::Csv { row_start, row_end }
            | SourceLocation::Xlsx {
                row_start, row_end, ..
            } => {
                self.fallback_row = self.fallback_row.max(row_end);
                (row_start, row_end)
            }
            _ => {
                self.fallback_row += 1;
                (self.fallback_row, self.fallback_row)
            }
        };
        let tokens = self.tokens.count(&block.text);
        if tokens <= self.body_max {
            return Some(Piece {
                row_start,
                row_end,
                text: block.text,
                tokens,
            });
        }
        self.queue = self.split(&block, row_start, row_end).into();
        self.queue.pop_front()
    }

    /// A record that does not fit one chunk, cut by fields into titled parts.
    fn split(&self, block: &ContentBlock, row_start: u32, row_end: u32) -> Vec<Piece> {
        let (base, lines) = record_lines(block, row_start);
        let widest_title =
            format!("{base} (parte {MAX_PARTS_PLACEHOLDER}/{MAX_PARTS_PLACEHOLDER}):");
        let budget = self
            .body_max
            .saturating_sub(self.tokens.count(&widest_title) + SEPARATOR_TOKENS)
            .max(1);

        // Lines too long for a part are cut into several.
        let mut fitted: Vec<String> = Vec::new();
        for line in lines {
            if self.tokens.count(&line) <= budget {
                fitted.push(line);
            } else {
                fitted.extend(split_text(&line, budget, self.tokens));
            }
        }
        let mut parts: Vec<Vec<String>> = Vec::new();
        let mut current: Vec<String> = Vec::new();
        let mut used = 0;
        for line in fitted {
            let cost = self.tokens.count(&line) + SEPARATOR_TOKENS;
            if !current.is_empty() && used + cost > budget {
                parts.push(std::mem::take(&mut current));
                used = 0;
            }
            used += cost;
            current.push(line);
        }
        if !current.is_empty() {
            parts.push(current);
        }
        let total = parts.len();
        parts
            .into_iter()
            .enumerate()
            .map(|(i, lines)| {
                let text = format!("{base} (parte {}/{total}):\n{}", i + 1, lines.join("\n"));
                Piece {
                    row_start,
                    row_end,
                    tokens: self.tokens.count(&text),
                    text,
                }
            })
            .collect()
    }

    /// Reads pieces into a group until the target is reached. The flag is `true` when the
    /// source is exhausted, i.e. this is the last group.
    fn build_group(&mut self) -> Option<(Group, bool)> {
        let mut group = Group::default();
        loop {
            let Some(piece) = self.take_piece() else {
                return (!group.pieces.is_empty()).then_some((group, true));
            };
            let added = piece.tokens
                + if group.pieces.is_empty() {
                    0
                } else {
                    SEPARATOR_TOKENS
                };
            if !group.pieces.is_empty() && group.tokens + added > self.body_target {
                self.pending = Some(piece);
                return Some((group, false));
            }
            group.push(piece);
        }
    }

    fn finish(&mut self, group: Group) -> DocumentChunk {
        let first = group.pieces.iter().map(|p| p.row_start).min().unwrap_or(0);
        let last = group
            .pieces
            .iter()
            .map(|p| p.row_end)
            .max()
            .unwrap_or(first);
        let body = group
            .pieces
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let text = format!("{}{body}", self.preamble(first, last));
        let index = self.next_index;
        self.next_index += 1;
        DocumentChunk {
            document_id: self.context.document_id,
            chunk_id: None,
            index,
            token_count: Some(self.tokens.count(&text)),
            section_path: match &self.target {
                Target::Csv => Vec::new(),
                Target::Sheet { name, .. } => vec![name.clone()],
            },
            location: match &self.target {
                Target::Csv => SourceLocation::Csv {
                    row_start: first,
                    row_end: last,
                },
                Target::Sheet { index, name } => SourceLocation::Xlsx {
                    sheet_index: *index,
                    sheet_name: name.clone(),
                    row_start: first,
                    row_end: last,
                },
            },
            metadata: self.context.metadata(
                match &self.target {
                    Target::Csv => DocumentType::Csv,
                    Target::Sheet { .. } => DocumentType::Xlsx,
                },
                self.columns.clone(),
            ),
            content_hash: sha256_hex(&text),
            text,
        }
    }
}

impl<I: Iterator<Item = ContentBlock>> Iterator for RecordChunks<'_, I> {
    type Item = DocumentChunk;

    fn next(&mut self) -> Option<DocumentChunk> {
        let (group, last) = match self.held.take() {
            Some(held) => held,
            None => self.build_group()?,
        };
        if last {
            return Some(self.finish(group));
        }
        // Look ahead: a last group too small to stand alone joins this one if it fits.
        match self.build_group() {
            Some((mut tail, true))
                if tail.tokens + self.overhead < self.min_tokens
                    && group.tokens + SEPARATOR_TOKENS + tail.tokens <= self.body_max =>
            {
                let mut merged = group;
                merged.merge(std::mem::take(&mut tail));
                Some(self.finish(merged))
            }
            ahead => {
                self.held = ahead;
                Some(self.finish(group))
            }
        }
    }
}

/// "a, b, c", or "a, b, … (+N colunas)" when the full list passes `budget` tokens.
fn columns_line(columns: &[String], budget: u32, tokens: &dyn TokenCounter) -> String {
    let full = columns.join(", ");
    if tokens.count(&full) <= budget {
        return full;
    }
    let suffix = |kept: usize| {
        format!(
            "{}, … (+{} colunas)",
            columns[..kept].join(", "),
            columns.len() - kept
        )
    };
    let kept = longest_fit(columns.len(), |n| {
        n == 0 || tokens.count(&suffix(n)) <= budget
    });
    if kept == 0 {
        format!("… ({} colunas)", columns.len())
    } else {
        suffix(kept)
    }
}

/// Title ("Registro 7") and body lines of a record, empty values left out and whitespace
/// collapsed. A block that is not a record is split into its own lines.
fn record_lines(block: &ContentBlock, row: u32) -> (String, Vec<String>) {
    if let ContentKind::Record { fields } = &block.kind {
        let lines = fields
            .iter()
            .map(|f| (f.name.as_str(), collapse(&f.value)))
            .filter(|(_, value)| !value.is_empty())
            .map(|(name, value)| format!("{}: {value}", collapse(name)))
            .collect();
        return (format!("Registro {row}"), lines);
    }
    let mut lines = block.text.lines().map(collapse).filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or_default();
    match first.strip_suffix(':') {
        Some(title) if title.starts_with("Registro") => (title.to_string(), lines.collect()),
        _ => (
            format!("Registro {row}"),
            std::iter::once(first).chain(lines).collect(),
        ),
    }
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Cuts `text` into pieces of at most `budget` tokens, at word boundaries; a single word
/// longer than the budget is cut by characters.
fn split_text(text: &str, budget: u32, tokens: &dyn TokenCounter) -> Vec<String> {
    let mut words: Vec<String> = text.split_whitespace().map(str::to_string).collect();
    let mut pieces = Vec::new();
    let mut start = 0;
    while start < words.len() {
        let rest = &words[start..];
        let k = longest_fit(rest.len(), |k| {
            k == 0 || tokens.count(&rest[..k].join(" ")) <= budget
        });
        if k > 0 {
            pieces.push(rest[..k].join(" "));
            start += k;
            continue;
        }
        // A word longer than the budget: cut it by characters in one pass over its characters
        // (never copying the rest of the word for each piece, which made a long unbroken value —
        // a base64 blob, minified JSON — quadratic).
        let chars: Vec<char> = words[start].chars().collect();
        let mut offset = 0;
        loop {
            let remaining = chars.len() - offset;
            let m = longest_fit(remaining, |m| {
                m == 0
                    || tokens.count(&chars[offset..offset + m].iter().collect::<String>()) <= budget
            })
            .max(1);
            if m < remaining {
                pieces.push(chars[offset..offset + m].iter().collect());
                offset += m;
            } else {
                // What is left fits: it goes back to the word runs, which may join it to the
                // words that follow (or, if even one character is over a tiny budget, stands alone).
                let rest: String = chars[offset..].iter().collect();
                if tokens.count(&rest) <= budget {
                    words[start] = rest;
                } else {
                    pieces.push(rest);
                    start += 1;
                }
                break;
            }
        }
    }
    pieces
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use nlmx_domain::parsed::{ColumnKind, DatasetColumn, RecordField};

    use super::*;
    use crate::HeuristicTokenCounter;

    fn context(file_name: &str) -> ChunkContext {
        ChunkContext {
            document_id: 7,
            document_title: Some("Dados".into()),
            file_name: Some(file_name.into()),
            language: None,
        }
    }

    fn dataset(columns: &[&str]) -> DatasetMetadata {
        DatasetMetadata {
            columns: columns
                .iter()
                .map(|name| DatasetColumn {
                    name: name.to_string(),
                    kind: ColumnKind::Text,
                })
                .collect(),
            delimiter: ',',
            has_header: true,
            row_count: None,
        }
    }

    /// A record block as the CSV parser builds it: "Registro N:" and one line per non-empty
    /// field.
    fn record(row: u32, fields: &[(&str, &str)]) -> ContentBlock {
        let mut text = format!("Registro {row}:");
        for (name, value) in fields.iter().filter(|(_, v)| !v.is_empty()) {
            text.push_str(&format!("\n{name}: {value}"));
        }
        ContentBlock {
            kind: ContentKind::Record {
                fields: fields
                    .iter()
                    .map(|(name, value)| RecordField {
                        name: name.to_string(),
                        value: value.to_string(),
                    })
                    .collect(),
            },
            text,
            location: SourceLocation::csv(row, row).unwrap(),
        }
    }

    fn policy(target: u32, max: u32, min: u32) -> ChunkPolicy {
        ChunkPolicy {
            target_tokens: target,
            max_tokens: max,
            overlap_tokens: 50,
            min_tokens: min,
        }
    }

    fn run(
        columns: &[&str],
        records: Vec<ContentBlock>,
        policy: &ChunkPolicy,
    ) -> Vec<DocumentChunk> {
        RecordChunker
            .chunk(
                &context("dados.csv"),
                &dataset(columns),
                records,
                policy,
                &HeuristicTokenCounter,
            )
            .collect()
    }

    fn rows(chunk: &DocumentChunk) -> (u32, u32) {
        match chunk.location {
            SourceLocation::Csv { row_start, row_end } => (row_start, row_end),
            ref other => panic!("not a CSV location: {other:?}"),
        }
    }

    fn people(count: u32) -> Vec<ContentBlock> {
        (1..=count)
            .map(|n| {
                record(
                    n,
                    &[
                        ("Nome", &format!("Pessoa número {n}")),
                        ("Idade", &format!("{}", 20 + n % 50)),
                        ("Cidade", "Fortaleza"),
                        ("Profissão", "Engenheira de produção"),
                    ],
                )
            })
            .collect()
    }

    const PEOPLE: [&str; 4] = ["Nome", "Idade", "Cidade", "Profissão"];

    #[test]
    fn the_example_becomes_one_self_contained_chunk() {
        let blocks = vec![
            record(
                1,
                &[
                    ("Nome", "João"),
                    ("Idade", "32"),
                    ("Cidade", "Fortaleza"),
                    ("Profissão", "Engenheiro"),
                ],
            ),
            record(
                2,
                &[
                    ("Nome", "Maria"),
                    ("Idade", "28"),
                    ("Cidade", "Recife"),
                    ("Profissão", "Designer"),
                ],
            ),
        ];
        let chunks = run(&PEOPLE, blocks, &ChunkPolicy::default());
        assert_eq!(chunks.len(), 1);
        let chunk = &chunks[0];
        assert_eq!(
            chunk.text,
            "Arquivo: dados.csv\n\
             Colunas: Nome, Idade, Cidade, Profissão\n\
             Linhas 1–2\n\
             \n\
             Registro 1:\n\
             Nome: João\n\
             Idade: 32\n\
             Cidade: Fortaleza\n\
             Profissão: Engenheiro\n\
             \n\
             Registro 2:\n\
             Nome: Maria\n\
             Idade: 28\n\
             Cidade: Recife\n\
             Profissão: Designer"
        );
        assert_eq!(rows(chunk), (1, 2));
        assert_eq!(chunk.index, 0);
        assert!(chunk.section_path.is_empty());
        assert_eq!(chunk.metadata.file_name.as_deref(), Some("dados.csv"));
        assert_eq!(chunk.metadata.columns, PEOPLE);
        assert_eq!(chunk.metadata.document_type, DocumentType::Csv);
        assert_eq!(chunk.document_id, 7);
        assert_eq!(chunk.chunk_id, None);
        assert_eq!(
            chunk.token_count,
            Some(HeuristicTokenCounter.count(&chunk.text))
        );
    }

    #[test]
    fn a_single_row_chunk_says_linha() {
        let chunks = run(&PEOPLE, people(1), &ChunkPolicy::default());
        assert!(
            chunks[0].text.contains("\nLinha 1\n\n"),
            "{}",
            chunks[0].text
        );
        assert_eq!(rows(&chunks[0]), (1, 1));
    }

    #[test]
    fn chunks_respect_the_target_and_the_maximum() {
        let policy = policy(120, 160, 20);
        let chunks = run(&PEOPLE, people(1000), &policy);
        assert!(chunks.len() > 10);
        for chunk in &chunks {
            assert!(
                chunk.token_count.unwrap() <= policy.max_tokens,
                "{}",
                chunk.token_count.unwrap()
            );
            assert!(chunk.text.starts_with("Arquivo: dados.csv\nColunas: "));
        }
        // All but the last are filled close to the target.
        for chunk in &chunks[..chunks.len() - 1] {
            assert!(
                chunk.token_count.unwrap() >= policy.target_tokens / 2,
                "{}",
                chunk.token_count.unwrap()
            );
        }
    }

    #[test]
    fn row_ranges_are_contiguous_and_do_not_overlap() {
        let chunks = run(&PEOPLE, people(1000), &policy(120, 160, 20));
        let mut expected = 1;
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.index as usize, i);
            let (start, end) = rows(chunk);
            assert_eq!(start, expected, "chunk {i}");
            assert!(end >= start);
            expected = end + 1;
        }
        assert_eq!(expected, 1001);
        // Every record appears exactly once.
        let all: String = chunks.iter().map(|c| c.text.as_str()).collect();
        for n in [1, 500, 1000] {
            assert_eq!(all.matches(&format!("Registro {n}:")).count(), 1);
        }
    }

    #[test]
    fn a_small_last_chunk_joins_the_previous_one_when_it_fits() {
        // Without fusion the 3 records would be 2 + 1; the tail is below `min` and fits.
        let blocks = people(3);
        let one = HeuristicTokenCounter.count(&blocks[0].text);
        let target = 4 * one;
        let fused = run(
            &PEOPLE,
            people(3),
            &policy(target, target + 2 * one, 2 * one),
        );
        assert_eq!(fused.len(), 1, "{fused:#?}");
        assert_eq!(rows(&fused[0]), (1, 3));

        // With a minimum of zero nothing is fused.
        let tight = policy(2 * one + 24, 2 * one + 30, 0);
        let separate = run(&PEOPLE, people(3), &tight);
        assert_eq!(separate.len(), 2, "{separate:#?}");
        assert_eq!(rows(&separate[1]), (3, 3));
    }

    #[test]
    fn a_tail_that_would_not_fit_stays_separate() {
        let policy = policy(60, 70, 1000);
        let chunks = run(&PEOPLE, people(40), &policy);
        assert!(chunks.len() > 2);
        for chunk in &chunks {
            assert!(chunk.token_count.unwrap() <= policy.max_tokens);
        }
    }

    #[test]
    fn a_giant_record_is_split_by_fields_with_the_preamble_in_each_part() {
        let names: Vec<String> = (1..=60).map(|i| format!("Campo{i}")).collect();
        let fields: Vec<(String, String)> = names
            .iter()
            .map(|n| {
                (
                    n.clone(),
                    format!("valor longo e detalhado do {n} neste registro"),
                )
            })
            .collect();
        let borrowed: Vec<(&str, &str)> = fields
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let blocks = vec![
            record(1, &[("Nome", "antes")]),
            record(7, &borrowed),
            record(8, &[("Nome", "depois")]),
        ];
        let policy = policy(150, 200, 10);
        let columns: Vec<&str> = names.iter().map(String::as_str).collect();
        let chunks = run(&columns, blocks, &policy);

        let parts: Vec<_> = chunks
            .iter()
            .filter(|c| c.text.contains("Registro 7 (parte"))
            .collect();
        assert!(parts.len() >= 3, "{}", parts.len());
        for (i, part) in parts.iter().enumerate() {
            assert!(part.text.starts_with("Arquivo: dados.csv\n"));
            assert!(
                part.text
                    .contains(&format!("(parte {}/{}):", i + 1, parts.len()))
            );
            // The last part may share its chunk with the next record.
            let (start, end) = rows(part);
            assert_eq!(start, 7);
            assert!(end == 7 || i + 1 == parts.len(), "part {i} spans {end}");
            assert!(
                part.token_count.unwrap() <= policy.max_tokens,
                "{}",
                part.token_count.unwrap()
            );
        }
        // No field is lost.
        let all: String = parts.iter().map(|p| p.text.as_str()).collect();
        for name in &names {
            assert!(all.contains(&format!("{name}: valor longo")), "{name}");
        }
        // The neighbours are still there.
        assert!(chunks.iter().any(|c| c.text.contains("Nome: antes")));
        assert!(chunks.iter().any(|c| c.text.contains("Nome: depois")));
    }

    #[test]
    fn a_cell_bigger_than_any_chunk_is_cut_by_words() {
        let long = "palavra ".repeat(2000);
        let blocks = vec![record(1, &[("Nome", "x"), ("Texto", long.trim())])];
        let policy = policy(100, 120, 10);
        let chunks = run(&["Nome", "Texto"], blocks, &policy);
        assert!(chunks.len() > 5);
        for chunk in &chunks {
            assert!(
                chunk.token_count.unwrap() <= policy.max_tokens,
                "{}",
                chunk.token_count.unwrap()
            );
            assert_eq!(rows(chunk), (1, 1));
        }
        let words: usize = chunks
            .iter()
            .map(|c| c.text.matches("palavra").count())
            .sum();
        assert_eq!(words, 2000);
    }

    /// A long value with no spaces (a base64 blob, minified JSON) must not make the cut quadratic:
    /// 2 MB took 45 s in a debug build when each piece copied the rest of the word.
    #[test]
    fn a_huge_unbroken_value_is_cut_in_linear_time() {
        let started = std::time::Instant::now();
        let blob = "x".repeat(2 * 1024 * 1024);
        let pieces = split_text(&blob, 200, &HeuristicTokenCounter);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert!(pieces.iter().all(|p| HeuristicTokenCounter.count(p) <= 200));
        assert_eq!(pieces.concat(), blob, "nothing is lost");
        // Words around it still join the pieces as before.
        let mixed = format!("antes {} depois", "y".repeat(5000));
        let pieces = split_text(&mixed, 200, &HeuristicTokenCounter);
        assert_eq!(pieces.join(" ").replace(' ', ""), mixed.replace(' ', ""));
    }

    #[test]
    fn a_single_unbroken_word_is_cut_by_characters() {
        let blob = "x".repeat(10_000);
        let chunks = run(
            &["Dados"],
            vec![record(1, &[("Dados", &blob)])],
            &policy(100, 120, 10),
        );
        assert!(chunks.len() > 5);
        let total: usize = chunks.iter().map(|c| c.text.matches('x').count()).sum();
        assert_eq!(total, 10_000);
    }

    #[test]
    fn a_long_columns_list_is_truncated_in_the_preamble() {
        let names: Vec<String> = (1..=300).map(|i| format!("coluna_{i}")).collect();
        let columns: Vec<&str> = names.iter().map(String::as_str).collect();
        let policy = policy(350, 512, 40);
        let chunks = run(&columns, vec![record(1, &[("coluna_1", "a")])], &policy);
        assert_eq!(chunks.len(), 1);
        let chunk = &chunks[0];
        let columns_line = chunk.text.lines().nth(1).unwrap();
        assert!(
            columns_line.starts_with("Colunas: coluna_1, coluna_2"),
            "{columns_line}"
        );
        assert!(columns_line.contains("… (+"), "{columns_line}");
        assert!(columns_line.ends_with(" colunas)"), "{columns_line}");
        assert!(HeuristicTokenCounter.count(columns_line) <= policy.max_tokens / 4 + 3);
        // The structured field keeps every column.
        assert_eq!(chunk.metadata.columns.len(), 300);
        assert_eq!(chunk.metadata.columns[299], "coluna_300");
    }

    #[test]
    fn output_is_deterministic_and_hashed_by_text() {
        let a = run(&PEOPLE, people(200), &policy(120, 160, 20));
        let b = run(&PEOPLE, people(200), &policy(120, 160, 20));
        assert_eq!(a, b);
        for chunk in &a {
            assert_eq!(chunk.content_hash, sha256_hex(&chunk.text));
        }
        let hashes: std::collections::HashSet<_> = a.iter().map(|c| &c.content_hash).collect();
        assert_eq!(hashes.len(), a.len(), "different rows, different hashes");
        assert_eq!(RecordChunker.version(), RecordChunker::VERSION);
    }

    #[test]
    fn empty_values_stay_out_and_whitespace_is_collapsed_when_splitting() {
        let blocks = vec![record(
            3,
            &[
                ("Nome", "Ana"),
                ("Obs", ""),
                ("Nota", "linha 1\n   linha 2"),
            ],
        )];
        let chunks = run(&["Nome", "Obs", "Nota"], blocks, &ChunkPolicy::default());
        assert_eq!(chunks.len(), 1);
        assert!(!chunks[0].text.contains("Obs:"));
        assert_eq!(chunks[0].metadata.columns, ["Nome", "Obs", "Nota"]);
    }

    #[test]
    fn nothing_in_nothing_out() {
        assert!(run(&PEOPLE, vec![], &ChunkPolicy::default()).is_empty());
    }

    #[test]
    fn blocks_that_are_not_records_are_chunked_by_their_text() {
        let block = ContentBlock {
            kind: ContentKind::Paragraph,
            text: "Registro 4:\nNome: Bia".into(),
            location: SourceLocation::csv(4, 4).unwrap(),
        };
        let chunks = run(&["Nome"], vec![block], &ChunkPolicy::default());
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.ends_with("Registro 4:\nNome: Bia"));
        assert_eq!(rows(&chunks[0]), (4, 4));
    }

    #[test]
    fn chunks_are_produced_lazily() {
        let pulled = Cell::new(0u32);
        let endless = std::iter::from_fn(|| {
            pulled.set(pulled.get() + 1);
            Some(record(
                pulled.get(),
                &[("Nome", "Fulana de Tal"), ("Idade", "30")],
            ))
        });
        let tokens = HeuristicTokenCounter;
        let mut chunks = RecordChunker.chunk(
            &context("infinito.csv"),
            &dataset(&["Nome", "Idade"]),
            endless,
            &ChunkPolicy::default(),
            &tokens,
        );
        let first = chunks
            .next()
            .expect("a first chunk without reading everything");
        assert_eq!(rows(&first).0, 1);
        let after_first = pulled.get();
        assert!(after_first < 400, "read ahead too much: {after_first}");
        chunks.next().unwrap();
        assert!(pulled.get() < after_first * 3, "{}", pulled.get());
    }

    #[test]
    fn a_big_stream_is_chunked_without_collecting_it() {
        const ROWS: u32 = 300_000;
        let records = (1..=ROWS).map(|n| {
            record(
                n,
                &[
                    ("Nome", "Fulano"),
                    ("Código", &n.to_string()),
                    ("Cidade", "Recife"),
                ],
            )
        });
        let tokens = HeuristicTokenCounter;
        let mut expected = 1;
        let mut count = 0;
        for chunk in RecordChunker.chunk(
            &context("grande.csv"),
            &dataset(&["Nome", "Código", "Cidade"]),
            records,
            &ChunkPolicy::default(),
            &tokens,
        ) {
            let (start, end) = rows(&chunk);
            assert_eq!(start, expected);
            assert!(chunk.token_count.unwrap() <= ChunkPolicy::default().max_tokens);
            expected = end + 1;
            count += 1;
        }
        assert_eq!(expected, ROWS + 1);
        assert!(count > 1000, "{count}");
    }
}
