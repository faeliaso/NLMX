//! What the PDF viewer opens: a document, a page and the boxes to highlight.

use crate::{
    document::{BoundingBox, TextSpan},
    ingestion::{DocumentId, PageBox},
    retrieval::fold,
};

#[derive(Debug, Clone, PartialEq)]
pub struct ViewerTarget {
    pub document_id: DocumentId,
    /// 1-based page to show first.
    pub page: u32,
    /// Boxes to highlight (PDF points, top-left origin); empty when no coordinates are known.
    pub highlights: Vec<PageBox>,
}

impl ViewerTarget {
    /// Where to scroll on `page`: the top of its first highlight, if any.
    pub fn anchor_top(&self) -> Option<f32> {
        self.highlights
            .iter()
            .filter(|b| b.page == self.page)
            .map(|b| b.bbox.top)
            .min_by(f32::total_cmp)
    }
}

/// Folds one character for matching (accents and case removed), keeping a 1:1 mapping.
fn fold_char(c: char) -> char {
    fold(&c.to_string()).chars().next().unwrap_or(c)
}

/// Occurrences of `query` in a page's text spans (accent/case-insensitive, whitespace
/// collapsed, matches may cross spans). Each occurrence is the boxes it covers: the part of
/// every span it touches, cut proportionally to the characters.
pub fn find_in_spans(spans: &[TextSpan], query: &str) -> Vec<Vec<BoundingBox>> {
    let needle: Vec<char> = query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .map(fold_char)
        .collect();
    if needle.is_empty() {
        return Vec::new();
    }
    // Page text with, for each char, (span index, char index in span); spans joined by a space.
    let mut chars: Vec<(char, Option<(usize, usize)>)> = Vec::new();
    for (i, span) in spans.iter().enumerate() {
        // A word hyphenated at the end of the previous line continues here: drop the hyphen.
        let continues = chars.last().is_some_and(|(c, _)| *c == '-')
            && chars.len() >= 2
            && chars[chars.len() - 2].0.is_alphanumeric()
            && span.text.chars().next().is_some_and(char::is_alphanumeric);
        if continues {
            chars.pop();
        } else if i > 0 && chars.last().is_some_and(|(c, _)| *c != ' ') {
            chars.push((' ', None));
        }
        let mut previous_space = chars.last().is_none_or(|(c, _)| *c == ' ');
        for (j, c) in span.text.chars().enumerate() {
            let c = if c.is_whitespace() { ' ' } else { fold_char(c) };
            if c == ' ' && previous_space {
                continue;
            }
            previous_space = c == ' ';
            chars.push((c, Some((i, j))));
        }
    }
    let mut hits = Vec::new();
    let mut start = 0;
    while start + needle.len() <= chars.len() {
        let matched = chars[start..start + needle.len()]
            .iter()
            .zip(&needle)
            .all(|((c, _), n)| c == n);
        if !matched {
            start += 1;
            continue;
        }
        let mut boxes: Vec<BoundingBox> = Vec::new();
        let mut current: Option<(usize, usize, usize)> = None; // (span, first, last)
        let flush = |current: Option<(usize, usize, usize)>, boxes: &mut Vec<BoundingBox>| {
            if let Some((i, a, b)) = current {
                let span = &spans[i];
                let len = span.text.chars().count().max(1) as f32;
                let w = span.bbox.right - span.bbox.left;
                boxes.push(BoundingBox {
                    left: span.bbox.left + w * a as f32 / len,
                    right: span.bbox.left + w * (b + 1) as f32 / len,
                    top: span.bbox.top,
                    bottom: span.bbox.bottom,
                });
            }
        };
        for (_, pos) in &chars[start..start + needle.len()] {
            let Some((i, j)) = *pos else { continue };
            current = match current {
                Some((ci, a, _)) if ci == i => Some((ci, a, j)),
                other => {
                    flush(other, &mut boxes);
                    Some((i, j, j))
                }
            };
        }
        flush(current, &mut boxes);
        hits.push(boxes);
        start += needle.len();
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str, left: f32, top: f32) -> TextSpan {
        TextSpan {
            text: text.into(),
            bbox: BoundingBox {
                left,
                top,
                right: left + text.chars().count() as f32 * 10.0,
                bottom: top + 12.0,
            },
            font_name: "Helvetica".into(),
            font_size: 11.0,
            bold: false,
            italic: false,
        }
    }

    #[test]
    fn finds_words_ignoring_accents_and_case_with_proportional_boxes() {
        let spans = [
            span("O prazo de Carência", 0.0, 100.0),
            span("termina; carencia.", 0.0, 120.0),
        ];
        let hits = find_in_spans(&spans, "carência");
        assert_eq!(hits.len(), 2);
        // "Carência" is chars 11..19 of the first span (10 pt per char).
        assert_eq!(
            hits[0],
            [BoundingBox {
                left: 110.0,
                top: 100.0,
                right: 190.0,
                bottom: 112.0
            }]
        );
        assert_eq!(hits[1][0].top, 120.0);
    }

    #[test]
    fn matches_across_spans_and_collapses_whitespace() {
        let spans = [
            span("o prazo de carência termina   após", 0.0, 0.0),
            span("cento e oitenta dias", 0.0, 20.0),
        ];
        let hits = find_in_spans(&spans, "após  cento e");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].len(), 2, "one box per span touched");
        assert_eq!(hits[0][1].left, 0.0);
        assert!(find_in_spans(&spans, "   ").is_empty());

        // Hyphenated at the end of a line: found across the two spans, without the hyphen.
        let hyphen = [
            span("período de carên-", 0.0, 0.0),
            span("cia aplicado", 0.0, 20.0),
        ];
        let hits = find_in_spans(&hyphen, "carência");
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0],
            [
                BoundingBox {
                    left: 110.0,
                    top: 0.0,
                    right: 160.0,
                    bottom: 12.0
                },
                BoundingBox {
                    left: 0.0,
                    top: 20.0,
                    right: 30.0,
                    bottom: 32.0
                },
            ]
        );
        assert_eq!(
            find_in_spans(
                &[span("bem-", 0.0, 0.0), span("vindo", 0.0, 20.0)],
                "bem vindo"
            )
            .len(),
            0
        );
        assert!(find_in_spans(&spans, "inexistente").is_empty());
    }

    #[test]
    fn anchors_at_the_first_highlight_of_the_page() {
        let b = |page, top| PageBox {
            page,
            bbox: BoundingBox {
                left: 0.0,
                top,
                right: 10.0,
                bottom: top + 10.0,
            },
        };
        let t = ViewerTarget {
            document_id: 1,
            page: 2,
            highlights: vec![b(1, 5.0), b(2, 300.0), b(2, 120.0)],
        };
        assert_eq!(t.anchor_top(), Some(120.0));
        assert_eq!(
            ViewerTarget {
                highlights: vec![],
                ..t
            }
            .anchor_top(),
            None
        );
    }
}
