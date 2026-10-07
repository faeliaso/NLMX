//! `StructureAnalyzer`: page layout → headings, paragraphs and lists; removes repeated
//! headers/footers and end-of-line hyphenation. Pure Rust, deterministic.

mod lines;
mod normalize;

use nlmx_application::ports::StructureAnalyzer;
use nlmx_domain::{
    document::BoundingBox,
    ingestion::{Block, BlockKind, PageBox, PageLayout, StructuredDocument},
};

use lines::Line;
pub use normalize::normalize;

/// Bump when the output for the same input changes (stored as `documents.extractor_version`).
pub const VERSION: u32 = 1;

/// Headings are at least this much larger than body text…
const HEADING_SIZE_RATIO: f32 = 1.15;
/// …or bold, short and without a final period.
const HEADING_MAX_CHARS: usize = 80;
/// Lines further apart than this many line heights start a new paragraph.
const PARAGRAPH_GAP_RATIO: f32 = 1.6;

#[derive(Debug, Default, Clone, Copy)]
pub struct HeuristicStructureAnalyzer;

impl StructureAnalyzer for HeuristicStructureAnalyzer {
    fn version(&self) -> u32 {
        VERSION
    }

    fn analyze(&self, pages: &[PageLayout]) -> StructuredDocument {
        let mut page_lines: Vec<Vec<Line>> = pages.iter().map(lines::from_page).collect();
        lines::remove_headers_and_footers(&mut page_lines, pages);

        let all: Vec<&Line> = page_lines.iter().flatten().collect();
        let body = lines::body_style(&all);
        let heading_sizes = heading_sizes(&all, &body);

        let mut builder = Builder::default();
        for line in all {
            let kind = classify(line, &body, &heading_sizes);
            builder.push(line, kind);
        }
        builder.finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum LineKind {
    Heading(u8),
    ListStart,
    Text,
}

fn is_heading(line: &Line, body: &lines::BodyStyle) -> bool {
    let chars = line.text.chars().count();
    if chars == 0 || chars > HEADING_MAX_CHARS * 2 {
        return false;
    }
    let larger = line.font_size >= body.font_size * HEADING_SIZE_RATIO;
    let emphasized = line.bold
        && !body.bold
        && chars <= HEADING_MAX_CHARS
        && !line.text.ends_with(['.', ',', ';']);
    larger || emphasized
}

/// Distinct heading sizes, largest first; a heading's level is its rank (1 = largest).
fn heading_sizes(lines: &[&Line], body: &lines::BodyStyle) -> Vec<f32> {
    let mut sizes: Vec<f32> = Vec::new();
    for line in lines.iter().filter(|l| is_heading(l, body)) {
        let size = round_size(line.font_size);
        if !sizes.iter().any(|s| (s - size).abs() < 0.01) {
            sizes.push(size);
        }
    }
    sizes.sort_by(|a, b| b.total_cmp(a));
    sizes
}

fn round_size(size: f32) -> f32 {
    (size * 2.0).round() / 2.0
}

fn classify(line: &Line, body: &lines::BodyStyle, heading_sizes: &[f32]) -> LineKind {
    if is_heading(line, body) {
        let size = round_size(line.font_size);
        let rank = heading_sizes
            .iter()
            .position(|s| (s - size).abs() < 0.01)
            .unwrap_or(0);
        return LineKind::Heading((rank + 1).min(6) as u8);
    }
    if starts_list_item(&line.text) {
        return LineKind::ListStart;
    }
    LineKind::Text
}

/// "• item", "- item", "– item", "1. item", "2) item", "a) item", "(b) item".
pub fn starts_list_item(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if matches!(first, '•' | '▪' | '◦' | '·' | '‣' | '-' | '–' | '—' | '*') {
        return chars.next().is_some_and(char::is_whitespace);
    }
    let rest = text.strip_prefix('(').unwrap_or(text);
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    let (marker_len, terminators): (usize, &[char]) = if (1..=3).contains(&digits) {
        (digits, &['.', ')'])
    } else if rest.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
        // Letters only with ")" so initials such as "A. Souza" are not list items.
        (1, &[')'])
    } else {
        return false;
    };
    let mut after = rest[marker_len..].chars();
    after.next().is_some_and(|c| terminators.contains(&c))
        && after.next().is_some_and(char::is_whitespace)
}

/// Accumulates lines into blocks and tracks the heading stack for section paths.
#[derive(Default)]
struct Builder {
    blocks: Vec<Block>,
    sections: Vec<(u8, String)>,
    /// The last line added to the open text block, for continuation checks.
    last_line: Option<LastLine>,
}

struct LastLine {
    page: u32,
    bbox: BoundingBox,
    font_size: f32,
    /// Whether the open block is a heading (merging consecutive heading lines).
    heading: Option<u8>,
}

impl Builder {
    fn section_path(&self) -> Vec<String> {
        self.sections
            .iter()
            .map(|(_, title)| title.clone())
            .collect()
    }

    fn push(&mut self, line: &Line, kind: LineKind) {
        if self.continues(line, kind) {
            let block = self
                .blocks
                .last_mut()
                .expect("continuation has an open block");
            join_text(&mut block.text, &line.text);
            add_box(&mut block.boxes, line.page, line.bbox);
            if let BlockKind::Heading { level } = block.kind {
                // A multi-line heading: keep the stack entry in sync.
                if let Some(entry) = self.sections.last_mut().filter(|(l, _)| *l == level) {
                    entry.1 = block.text.clone();
                }
            }
        } else {
            let block_kind = match kind {
                LineKind::Heading(level) => {
                    while self.sections.last().is_some_and(|(l, _)| *l >= level) {
                        self.sections.pop();
                    }
                    BlockKind::Heading { level }
                }
                LineKind::ListStart => BlockKind::ListItem,
                LineKind::Text => BlockKind::Paragraph,
            };
            self.blocks.push(Block {
                kind: block_kind,
                text: line.text.clone(),
                page: line.page,
                boxes: vec![PageBox {
                    page: line.page,
                    bbox: line.bbox,
                }],
                section_path: self.section_path(),
            });
            if let LineKind::Heading(level) = kind {
                self.sections.push((level, line.text.clone()));
            }
        }
        self.last_line = Some(LastLine {
            page: line.page,
            bbox: line.bbox,
            font_size: line.font_size,
            heading: match kind {
                LineKind::Heading(level) => Some(level),
                _ => None,
            },
        });
    }

    /// Whether `line` continues the open block instead of starting a new one.
    fn continues(&self, line: &Line, kind: LineKind) -> bool {
        let (Some(last), Some(block)) = (&self.last_line, self.blocks.last()) else {
            return false;
        };
        let similar_size = (last.font_size - line.font_size).abs() < 1.0;
        let close = if line.page == last.page {
            let gap = line.bbox.top - last.bbox.bottom;
            let line_height = last.bbox.height().max(1.0);
            gap <= line_height * (PARAGRAPH_GAP_RATIO - 1.0) + 0.5 && gap > -line_height
        } else {
            // A paragraph that continues on the next page: previous line had no final punctuation.
            line.page == last.page + 1 && !block.text.ends_with(['.', '!', '?', ':'])
        };
        match (block.kind, kind) {
            (BlockKind::Heading { level }, LineKind::Heading(next)) => {
                level == next && last.heading == Some(level) && line.page == last.page && close
            }
            (BlockKind::Paragraph | BlockKind::ListItem, LineKind::Text) => similar_size && close,
            _ => false,
        }
    }

    fn finish(self) -> StructuredDocument {
        StructuredDocument {
            blocks: self.blocks,
        }
    }
}

/// Joins a wrapped line, removing end-of-line hyphenation ("carên-" + "cia" → "carência").
fn join_text(text: &mut String, next: &str) {
    let hyphenated = text.ends_with('-')
        && text.chars().rev().nth(1).is_some_and(char::is_alphabetic)
        && next.chars().next().is_some_and(char::is_lowercase);
    if hyphenated {
        text.pop();
    } else if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(next);
}

/// Adds a line box, merging it into the block's box on the same page.
fn add_box(boxes: &mut Vec<PageBox>, page: u32, bbox: BoundingBox) {
    match boxes.iter_mut().find(|b| b.page == page) {
        Some(existing) => existing.bbox = existing.bbox.union(&bbox),
        None => boxes.push(PageBox { page, bbox }),
    }
}

#[cfg(test)]
mod tests;
