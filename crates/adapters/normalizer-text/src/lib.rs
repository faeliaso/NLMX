//! `DocumentNormalizer` adapter: the same text clean-up for every format.
//!
//! It works on the text of blocks and titles of a [`ParsedDocument`] and nothing else: kinds,
//! levels, languages, pages, warnings and every [`SourceLocation`] are left as they are (a TXT
//! location points into the decoded source, which normalizing a block's text does not change).
//! PDF-specific clean-up that needs coordinates (hyphenation at line ends, running headers and
//! footers) stays in the structure analyzer; this stage only sees text, so it is safe on a PDF
//! document too. The result is idempotent.

use nlmx_application::ports::DocumentNormalizer;
use nlmx_domain::parsed::{
    ContentBlock, ContentKind, DatasetMetadata, DocumentMetadata, DocumentSection, ParsedDocument,
    RecordField,
};
use unicode_normalization::UnicodeNormalization;

/// Unicode form, stray characters and spacing. See the crate documentation.
#[derive(Debug, Clone, Copy, Default)]
pub struct TextNormalizer;

impl TextNormalizer {
    /// Bumped whenever the output for the same input changes.
    pub const VERSION: u32 = 1;
}

impl DocumentNormalizer for TextNormalizer {
    fn version(&self) -> u32 {
        Self::VERSION
    }

    /// `document` must be valid (as every `ParsedDocument` built with `new` is): the result
    /// keeps its locations, so it is valid too.
    fn normalize(&self, document: ParsedDocument) -> ParsedDocument {
        let (kind, metadata, pages, sections, warnings) = document.into_parts();
        let sections = sections.into_iter().filter_map(normalize_section).collect();
        ParsedDocument::new(
            kind,
            normalize_metadata(metadata),
            pages,
            sections,
            warnings,
        )
        .expect("normalizing text keeps locations, so a valid document stays valid")
    }
}

fn normalize_section(section: DocumentSection) -> Option<DocumentSection> {
    let title = section
        .title
        .map(|t| single_line(&t))
        .filter(|t| !t.is_empty());
    let path = section
        .path
        .iter()
        .map(|p| single_line(p))
        .filter(|p| !p.is_empty())
        .collect();
    let blocks = section
        .blocks
        .into_iter()
        .filter_map(normalize_block)
        .collect();
    DocumentSection::new(title, section.level, path, blocks)
}

fn normalize_block(block: ContentBlock) -> Option<ContentBlock> {
    let (kind, text) = match block.kind {
        ContentKind::CodeBlock { language } => {
            (ContentKind::CodeBlock { language }, code(&block.text))
        }
        ContentKind::Table { header, rows } => (
            ContentKind::Table {
                header: header.iter().map(|c| single_line(c)).collect(),
                rows: rows
                    .iter()
                    .map(|row| row.iter().map(|c| single_line(c)).collect())
                    .collect(),
            },
            lines(&block.text),
        ),
        ContentKind::Record { fields } => (
            ContentKind::Record {
                fields: fields
                    .into_iter()
                    .map(|f| RecordField {
                        name: single_line(&f.name),
                        value: single_line(&f.value),
                    })
                    .collect(),
            },
            lines(&block.text),
        ),
        other => (other, single_line(&block.text)),
    };
    (!text.trim().is_empty()).then_some(ContentBlock {
        kind,
        text,
        location: block.location,
    })
}

fn normalize_metadata(mut metadata: DocumentMetadata) -> DocumentMetadata {
    let field = |value: Option<String>| value.map(|v| single_line(&v)).filter(|v| !v.is_empty());
    metadata.title = field(metadata.title);
    metadata.author = field(metadata.author);
    metadata.subject = field(metadata.subject);
    metadata.publisher = field(metadata.publisher);
    metadata.dataset = metadata.dataset.map(|mut dataset: DatasetMetadata| {
        for column in &mut dataset.columns {
            let name = single_line(&column.name);
            if !name.is_empty() {
                column.name = name;
            }
        }
        dataset
    });
    metadata
}

/// How much of the text is rewritten.
#[derive(Clone, Copy)]
struct Mode {
    /// Unicode spaces become `' '` and ligatures are expanded.
    fold: bool,
}

/// Characters that are dropped: zero-width ones, the soft hyphen and controls (but not `\n`,
/// `\t`; carriage returns are handled as line ends before this).
fn is_dropped(c: char) -> bool {
    matches!(c, '\u{feff}' | '\u{200b}' | '\u{2060}' | '\u{00ad}')
        || (c.is_control() && c != '\n' && c != '\t')
}

fn is_unicode_space(c: char) -> bool {
    matches!(
        c,
        '\u{00a0}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
    )
}

/// Line ends to `\n`, dropped characters removed, and, when folding, exotic spaces and
/// ligatures replaced. NFC is applied by the callers once the spacing is final.
fn clean(text: &str, mode: Mode) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\u{2028}' | '\u{2029}' => out.push('\n'),
            c if is_dropped(c) => {}
            c if mode.fold && is_unicode_space(c) => out.push(' '),
            'ﬀ' if mode.fold => out.push_str("ff"),
            'ﬁ' if mode.fold => out.push_str("fi"),
            'ﬂ' if mode.fold => out.push_str("fl"),
            'ﬃ' if mode.fold => out.push_str("ffi"),
            'ﬄ' if mode.fold => out.push_str("ffl"),
            'ﬅ' | 'ﬆ' if mode.fold => out.push_str("st"),
            c => out.push(c),
        }
    }
    out
}

fn nfc(text: &str) -> String {
    text.nfc().collect()
}

/// Everything on one line: whitespace runs (newlines included) become one space.
fn single_line(text: &str) -> String {
    let cleaned = clean(text, Mode { fold: true });
    nfc(&cleaned.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Line by line: spaces collapsed inside each line, blank lines dropped, line breaks kept.
fn lines(text: &str) -> String {
    let cleaned = clean(text, Mode { fold: true });
    nfc(&cleaned
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Code is kept as written (indentation, blank lines, spaces); only dropped characters go.
fn code(text: &str) -> String {
    nfc(&clean(text, Mode { fold: false }))
}

#[cfg(test)]
mod tests {
    use nlmx_domain::{
        document_type::DocumentType,
        parsed::{ColumnKind, DatasetColumn, PageSummary, ParseWarning},
        source::SourceLocation,
    };

    use super::*;

    fn md_at(line: u32) -> SourceLocation {
        SourceLocation::markdown(vec!["Guia".into()], Some((line, line + 1))).unwrap()
    }

    fn block(kind: ContentKind, text: &str, location: SourceLocation) -> ContentBlock {
        ContentBlock {
            kind,
            text: text.into(),
            location,
        }
    }

    fn paragraph(text: &str) -> ContentBlock {
        block(ContentKind::Paragraph, text, md_at(1))
    }

    /// A one-section Markdown document.
    fn doc(blocks: Vec<ContentBlock>) -> ParsedDocument {
        ParsedDocument::new(
            DocumentType::Markdown,
            DocumentMetadata::default(),
            vec![],
            vec![
                DocumentSection::new(Some("Guia".into()), 1, vec!["Guia".into()], blocks).unwrap(),
            ],
            vec![],
        )
        .unwrap()
    }

    fn normalize(document: ParsedDocument) -> ParsedDocument {
        TextNormalizer.normalize(document)
    }

    fn texts(document: &ParsedDocument) -> Vec<String> {
        document.blocks().map(|b| b.text.clone()).collect()
    }

    fn one(text: &str) -> String {
        texts(&normalize(doc(vec![paragraph(text)])))
            .pop()
            .unwrap_or_default()
    }

    #[test]
    fn the_version_is_exposed() {
        assert_eq!(TextNormalizer.version(), TextNormalizer::VERSION);
    }

    #[test]
    fn text_is_normalized_to_nfc() {
        assert_eq!(one("Joa\u{303}o e A\u{301}gua"), "João e Água");
        assert_eq!(one("João"), "João", "already composed text is untouched");
    }

    #[test]
    fn zero_width_soft_hyphen_bom_and_controls_are_removed() {
        assert_eq!(one("\u{feff}ca\u{200b}sa\u{2060}"), "casa");
        assert_eq!(one("inter\u{ad}nacional"), "internacional");
        assert_eq!(one("a\u{0}b\u{7}c\u{85}d"), "abcd");
        // Removing the invisible character first lets the accent compose.
        assert_eq!(one("o\u{200b}\u{303}"), "õ");
    }

    #[test]
    fn exotic_spaces_become_one_space() {
        assert_eq!(
            one("a\u{a0}b\u{2003}c\u{202f}d\u{3000}e\u{205f}f"),
            "a b c d e f"
        );
        assert_eq!(one("  a \t\t b  "), "a b");
    }

    #[test]
    fn ligatures_are_expanded() {
        assert_eq!(
            one("ﬁnal ﬂuxo e\u{fb00}ect o\u{fb03}ce ba\u{fb04}e ﬅ"),
            "final fluxo effect office baffle st"
        );
    }

    #[test]
    fn newlines_collapse_in_flowing_text_and_headings() {
        assert_eq!(
            one("linha um\r\nlinha  dois\n\n\ntrês"),
            "linha um linha dois três"
        );
        let heading = block(
            ContentKind::Heading { level: 2 },
            "  Modelos\n Multilíngues ",
            md_at(3),
        );
        let out = normalize(doc(vec![heading]));
        assert_eq!(texts(&out), ["Modelos Multilíngues"]);
        let item = block(
            ContentKind::ListItem {
                ordered: true,
                depth: 1,
            },
            "um\u{a0}item\n continua",
            md_at(4),
        );
        let out = normalize(doc(vec![item]));
        assert_eq!(texts(&out), ["um item continua"]);
        assert_eq!(
            out.blocks().next().unwrap().kind,
            ContentKind::ListItem {
                ordered: true,
                depth: 1
            }
        );
    }

    #[test]
    fn code_keeps_its_indentation_and_lines() {
        let source = "fn main() {\r\n    let x = \"a\u{a0}b\";\r\n\r\n\t// ﬁm\r\n}\u{200b}";
        let code = block(
            ContentKind::CodeBlock {
                language: Some("rust".into()),
            },
            source,
            md_at(5),
        );
        let out = normalize(doc(vec![code]));
        assert_eq!(
            texts(&out),
            ["fn main() {\n    let x = \"a\u{a0}b\";\n\n\t// ﬁm\n}"],
            "spaces, blank lines, ligatures and NBSP stay; CRLF and zero-width go"
        );
        assert_eq!(
            out.blocks().next().unwrap().kind,
            ContentKind::CodeBlock {
                language: Some("rust".into())
            }
        );
    }

    #[test]
    fn records_keep_their_line_breaks_and_fields_are_normalized() {
        let location = SourceLocation::csv(1, 1).unwrap();
        let record = block(
            ContentKind::Record {
                fields: vec![
                    RecordField {
                        name: " Nome\u{a0}".into(),
                        value: "Joa\u{303}o\nSilva".into(),
                    },
                    RecordField {
                        name: "Idade".into(),
                        value: String::new(),
                    },
                ],
            },
            "Registro 1:\n  Nome:   Joa\u{303}o Silva \n\n\u{a0}\nCidade:\u{200b} Recife",
            location.clone(),
        );
        let parsed = ParsedDocument::new(
            DocumentType::Csv,
            DocumentMetadata::default(),
            vec![],
            vec![DocumentSection::new(None, 0, vec![], vec![record]).unwrap()],
            vec![],
        )
        .unwrap();
        let out = normalize(parsed);
        let block = out.blocks().next().unwrap();
        assert_eq!(block.text, "Registro 1:\nNome: João Silva\nCidade: Recife");
        assert_eq!(
            block.kind,
            ContentKind::Record {
                fields: vec![
                    RecordField {
                        name: "Nome".into(),
                        value: "João Silva".into(),
                    },
                    RecordField {
                        name: "Idade".into(),
                        value: String::new(),
                    },
                ],
            }
        );
        assert_eq!(block.location, location);
    }

    #[test]
    fn tables_keep_rows_and_normalize_cells() {
        let table = block(
            ContentKind::Table {
                header: vec![" a\u{a0}".into(), "b".into()],
                rows: vec![vec!["1 ".into(), "x\ny".into()]],
            },
            "a | b\n1  |  x y\n\n",
            md_at(7),
        );
        let out = normalize(doc(vec![table]));
        let block = out.blocks().next().unwrap();
        assert_eq!(block.text, "a | b\n1 | x y");
        assert_eq!(
            block.kind,
            ContentKind::Table {
                header: vec!["a".into(), "b".into()],
                rows: vec![vec!["1".into(), "x y".into()]],
            }
        );
    }

    #[test]
    fn titles_paths_and_metadata_are_normalized() {
        let section = DocumentSection::new(
            Some("  Cap\u{ad}ítulo\u{a0}3 ".into()),
            1,
            vec![
                "Cap\u{ad}ítulo\u{a0}3".into(),
                " ".into(),
                "Embeddings\n".into(),
            ],
            vec![paragraph("texto")],
        )
        .unwrap();
        let metadata = DocumentMetadata {
            title: Some(" Livro\u{200b} ".into()),
            author: Some("\u{200b}".into()),
            subject: Some("A\u{a0}B".into()),
            publisher: Some("E\u{303}d".into()),
            language: Some("pt-BR".into()),
            created_at: Some("2024-01-01".into()),
            dataset: Some(DatasetMetadata {
                columns: vec![
                    DatasetColumn {
                        name: " Preço\u{a0}".into(),
                        kind: ColumnKind::Number,
                    },
                    DatasetColumn {
                        name: "\u{200b}".into(),
                        kind: ColumnKind::Text,
                    },
                ],
                delimiter: ';',
                has_header: true,
                row_count: Some(3),
            }),
            ..Default::default()
        };
        let parsed = ParsedDocument::new(
            DocumentType::Markdown,
            metadata,
            vec![],
            vec![section],
            vec![],
        )
        .unwrap();
        let out = normalize(parsed);
        let section = &out.sections()[0];
        assert_eq!(section.title.as_deref(), Some("Capítulo 3"));
        assert_eq!(section.path, ["Capítulo 3", "Embeddings"]);
        let metadata = out.metadata();
        assert_eq!(metadata.title.as_deref(), Some("Livro"));
        assert_eq!(metadata.author, None, "a name with nothing left is dropped");
        assert_eq!(metadata.subject.as_deref(), Some("A B"));
        assert_eq!(metadata.publisher.as_deref(), Some("Ẽd"));
        assert_eq!(metadata.language.as_deref(), Some("pt-BR"));
        assert_eq!(metadata.created_at.as_deref(), Some("2024-01-01"));
        let dataset = metadata.dataset.as_ref().unwrap();
        assert_eq!(dataset.columns[0].name, "Preço");
        assert_eq!(
            dataset.columns[1].name, "\u{200b}",
            "an empty name is left as it was"
        );
        assert_eq!(dataset.row_count, Some(3));
    }

    #[test]
    fn empty_blocks_are_dropped_and_empty_sections_disappear() {
        let first = SourceLocation::markdown(vec!["A".into()], Some((1, 1))).unwrap();
        let second = SourceLocation::markdown(vec!["A".into()], Some((9, 9))).unwrap();
        let empty_only = DocumentSection::new(
            None,
            0,
            vec![],
            vec![block(
                ContentKind::Paragraph,
                "\u{200b} \u{a0}",
                first.clone(),
            )],
        )
        .unwrap();
        let mixed = DocumentSection::new(
            Some("B".into()),
            1,
            vec!["B".into()],
            vec![
                block(ContentKind::Paragraph, "\u{feff}", first),
                block(ContentKind::Paragraph, "fica", second.clone()),
            ],
        )
        .unwrap();
        let parsed = ParsedDocument::new(
            DocumentType::Markdown,
            DocumentMetadata::default(),
            vec![],
            vec![empty_only, mixed],
            vec![],
        )
        .unwrap();
        let out = normalize(parsed);
        assert_eq!(out.sections().len(), 1);
        assert_eq!(texts(&out), ["fica"]);
        // The section now starts at its first remaining block.
        assert_eq!(out.sections()[0].location, second);
        assert!(out.has_text());
    }

    #[test]
    fn locations_kinds_pages_and_warnings_are_untouched() {
        let boxed = SourceLocation::pdf(
            2,
            3,
            vec![nlmx_domain::ingestion::PageBox {
                page: 2,
                bbox: nlmx_domain::document::BoundingBox {
                    left: 1.0,
                    top: 2.0,
                    right: 3.0,
                    bottom: 4.0,
                },
            }],
        )
        .unwrap();
        let pages = vec![PageSummary {
            number: 1,
            width: 612.0,
            height: 792.0,
            char_count: 5,
            has_text: true,
        }];
        let warnings = vec![
            ParseWarning::FallbackEncoding,
            ParseWarning::SkippedUnit { index: 4 },
        ];
        let section = DocumentSection::new(
            Some("Introdução".into()),
            2,
            vec!["Introdução".into()],
            vec![
                block(
                    ContentKind::Heading { level: 2 },
                    "Introdução",
                    boxed.clone(),
                ),
                block(
                    ContentKind::Paragraph,
                    "Texto\u{a0}com  espaços",
                    boxed.clone(),
                ),
            ],
        )
        .unwrap();
        let parsed = ParsedDocument::new(
            DocumentType::Pdf,
            DocumentMetadata {
                page_count: Some(1),
                source_version: Some("1.7".into()),
                ..Default::default()
            },
            pages.clone(),
            vec![section],
            warnings.clone(),
        )
        .unwrap();
        let out = normalize(parsed);
        assert_eq!(out.document_type(), DocumentType::Pdf);
        assert_eq!(out.pages(), pages);
        assert_eq!(out.warnings(), warnings);
        assert_eq!(out.metadata().page_count, Some(1));
        assert_eq!(out.metadata().source_version.as_deref(), Some("1.7"));
        let blocks: Vec<_> = out.blocks().collect();
        assert_eq!(blocks[0].kind, ContentKind::Heading { level: 2 });
        assert_eq!(blocks[1].text, "Texto com espaços");
        assert!(blocks.iter().all(|b| b.location == boxed));
        assert_eq!(out.sections()[0].level, 2);
        assert_eq!(out.validate(), Ok(()));
    }

    #[test]
    fn already_clean_documents_are_returned_equal() {
        let clean = doc(vec![
            paragraph("Texto limpo, com acentuação."),
            block(ContentKind::Heading { level: 2 }, "Título", md_at(3)),
        ]);
        assert_eq!(normalize(clean.clone()), clean);
    }

    /// A deterministic stream of odd strings built from characters that exercise every rule.
    fn odd_strings(count: usize) -> Vec<String> {
        const POOL: &[&str] = &[
            "a", "Z", "ç", "ã", "o\u{303}", "e\u{301}", "é", " ", "  ", "\t", "\n", "\r\n", "\r",
            "\u{a0}", "\u{2003}", "\u{3000}", "\u{200b}", "\u{feff}", "\u{ad}", "\u{0}", "\u{7}",
            "\u{2028}", "ﬁ", "ﬃ", "ﬅ", "日本", "—", "-", ".", "1", "R$", "\u{301}", "\u{200d}",
        ];
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        (0..count)
            .map(|_| {
                let length = (next() % 24) as usize;
                (0..length)
                    .map(|_| POOL[(next() % POOL.len() as u64) as usize])
                    .collect()
            })
            .collect()
    }

    #[test]
    fn normalizing_twice_changes_nothing() {
        for text in odd_strings(400) {
            for kind in [
                ContentKind::Paragraph,
                ContentKind::Heading { level: 3 },
                ContentKind::ListItem {
                    ordered: false,
                    depth: 0,
                },
                ContentKind::CodeBlock { language: None },
                ContentKind::Table {
                    header: vec![text.clone()],
                    rows: vec![vec![text.clone()]],
                },
                ContentKind::Record {
                    fields: vec![RecordField {
                        name: text.clone(),
                        value: text.clone(),
                    }],
                },
            ] {
                // Keep a visible character so the block survives.
                let probe = format!("x{text}");
                let build = || doc(vec![block(kind.clone(), &probe, md_at(1))]);
                let once = normalize(build());
                let twice = normalize(once.clone());
                assert_eq!(once, twice, "{kind:?} {probe:?}");
                assert_eq!(once.validate(), Ok(()));
                for block in once.blocks() {
                    assert!(
                        !block.text.chars().any(|c| matches!(
                            c,
                            '\u{feff}' | '\u{200b}' | '\u{ad}' | '\u{0}' | '\r'
                        ))
                    );
                    assert!(!block.text.trim().is_empty());
                }
            }
        }
    }

    #[test]
    fn normalization_never_invents_text() {
        // Every non-space character of the output (decomposed, so an accent that composed
        // after an invisible character was removed counts as its parts) was in the input.
        for text in odd_strings(300) {
            let probe = format!("x{text}");
            let out = one(&probe);
            let allowed: String = probe.nfd().chain("fistfl".chars()).collect();
            for c in out.nfd().filter(|c| !c.is_whitespace()) {
                assert!(allowed.contains(c), "{c:?} from {probe:?}");
            }
        }
    }
}
