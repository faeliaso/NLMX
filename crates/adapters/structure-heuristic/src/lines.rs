//! Spans → lines, body style and repeated header/footer removal.

use std::collections::{HashMap, HashSet};

use nlmx_domain::{document::BoundingBox, ingestion::PageLayout};

use crate::normalize;

/// Fraction of the page height considered header (top) or footer (bottom) territory.
const MARGIN_BAND: f32 = 0.10;
/// A margin line repeated on at least this fraction of pages (and ≥ 3 pages) is a header/footer.
const REPEAT_RATIO: f32 = 0.5;
const REPEAT_MIN_PAGES: usize = 3;

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub page: u32,
    pub text: String,
    pub bbox: BoundingBox,
    pub font_size: f32,
    pub bold: bool,
}

/// Normalizes a span, keeping a line-ending soft hyphen as "-" so de-hyphenation can see it.
fn normalize_span(text: &str) -> String {
    let trimmed = text.trim_end();
    match trimmed.strip_suffix('\u{00AD}') {
        Some(rest) => format!("{}-", normalize(rest)),
        None => normalize(text),
    }
}

/// Groups a page's spans into lines in reading order (top to bottom, then left to right).
pub fn from_page(page: &PageLayout) -> Vec<Line> {
    let mut spans: Vec<_> = page
        .spans
        .iter()
        .map(|s| (s, normalize_span(&s.text)))
        .filter(|(_, text)| !text.is_empty())
        .collect();
    spans.sort_by(|(a, _), (b, _)| {
        a.bbox
            .top
            .total_cmp(&b.bbox.top)
            .then(a.bbox.left.total_cmp(&b.bbox.left))
    });

    let mut lines: Vec<(Line, usize, usize)> = Vec::new(); // line, bold chars, total chars
    for (span, text) in spans {
        let center = (span.bbox.top + span.bbox.bottom) / 2.0;
        let chars = text.chars().count();
        let joined = lines.iter_mut().rev().take(3).find(|(line, _, _)| {
            let line_center = (line.bbox.top + line.bbox.bottom) / 2.0;
            (line_center - center).abs() < span.font_size.min(line.font_size).max(1.0) * 0.5
        });
        match joined {
            Some((line, bold_chars, total)) => {
                // Insert in left-to-right order; spans were sorted by top first.
                if span.bbox.left >= line.bbox.right - 0.5 {
                    line.text.push(' ');
                    line.text.push_str(&text);
                } else {
                    line.text = format!("{text} {}", line.text);
                }
                line.bbox = line.bbox.union(&span.bbox);
                line.font_size = line.font_size.max(span.font_size);
                if span.bold {
                    *bold_chars += chars;
                }
                *total += chars;
            }
            None => lines.push((
                Line {
                    page: page.number,
                    text,
                    bbox: span.bbox,
                    font_size: span.font_size,
                    bold: false,
                },
                if span.bold { chars } else { 0 },
                chars,
            )),
        }
    }
    lines
        .into_iter()
        .map(|(mut line, bold, total)| {
            line.bold = total > 0 && bold * 2 > total;
            line.text = normalize(&line.text);
            line
        })
        .collect()
}

/// Key used to compare margin lines across pages: case-folded, digits replaced.
fn margin_key(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_ascii_digit() { '#' } else { c })
        .flat_map(char::to_lowercase)
        .collect()
}

fn in_margin(line: &Line, page_height: f32) -> bool {
    let center = (line.bbox.top + line.bbox.bottom) / 2.0;
    center <= page_height * MARGIN_BAND || center >= page_height * (1.0 - MARGIN_BAND)
}

/// "12", "- 12 -", "Página 3", "Página 3 de 10", "Page 3 of 10", "3/10".
pub fn is_page_number(text: &str) -> bool {
    let lower = text.to_lowercase();
    let mut rest = lower.trim_matches(|c: char| c == '-' || c == '–' || c.is_whitespace());
    for prefix in ["página", "pagina", "pág.", "pág", "page", "p."] {
        if let Some(stripped) = rest.strip_prefix(prefix) {
            rest = stripped.trim_start();
            break;
        }
    }
    let mut parts = rest
        .split(|c: char| c == '/' || c.is_whitespace())
        .filter(|p| !p.is_empty());
    let first = parts.next();
    let digits = |p: &str| !p.is_empty() && p.len() <= 5 && p.chars().all(|c| c.is_ascii_digit());
    match (first, parts.next(), parts.next(), parts.next()) {
        (Some(n), None, None, None) => digits(n),
        (Some(n), Some(total), None, None) => digits(n) && digits(total),
        (Some(n), Some("de" | "of"), Some(total), None) => digits(n) && digits(total),
        _ => false,
    }
}

/// Removes page numbers and lines repeated in the top/bottom margins across pages.
pub fn remove_headers_and_footers(pages: &mut [Vec<Line>], layouts: &[PageLayout]) {
    let heights: Vec<f32> = layouts.iter().map(|p| p.height).collect();
    let mut pages_with_key: HashMap<String, HashSet<usize>> = HashMap::new();
    for (index, lines) in pages.iter().enumerate() {
        for line in lines.iter().filter(|l| in_margin(l, heights[index])) {
            pages_with_key
                .entry(margin_key(&line.text))
                .or_default()
                .insert(index);
        }
    }
    let min_pages = ((pages.len() as f32 * REPEAT_RATIO).ceil() as usize).max(REPEAT_MIN_PAGES);
    let repeated: HashSet<&String> = pages_with_key
        .iter()
        .filter(|(_, p)| p.len() >= min_pages)
        .map(|(key, _)| key)
        .collect();

    for (index, lines) in pages.iter_mut().enumerate() {
        let height = heights[index];
        lines.retain(|line| {
            !(in_margin(line, height)
                && (is_page_number(&line.text) || repeated.contains(&margin_key(&line.text))))
        });
    }
}

/// The dominant (most characters) font size and weight of the document.
pub struct BodyStyle {
    pub font_size: f32,
    pub bold: bool,
}

pub fn body_style(lines: &[&Line]) -> BodyStyle {
    let mut by_size: HashMap<(i32, bool), usize> = HashMap::new();
    for line in lines {
        *by_size
            .entry(((line.font_size * 2.0).round() as i32, line.bold))
            .or_default() += line.text.chars().count();
    }
    // Ties broken by smaller size, then non-bold, so the result is deterministic.
    by_size
        .into_iter()
        .max_by(|(ka, a), (kb, b)| a.cmp(b).then(kb.0.cmp(&ka.0)).then(kb.1.cmp(&ka.1)))
        .map(|((size, bold), _)| BodyStyle {
            font_size: size as f32 / 2.0,
            bold,
        })
        .unwrap_or(BodyStyle {
            font_size: 11.0,
            bold: false,
        })
}
