//! PDFium-independent helpers: grouping glyphs into spans and parsing PDF dates.

use nlmx_domain::document::{BoundingBox, TextSpan};

/// One character as reported by the engine, already in top-left coordinates.
pub struct Glyph {
    pub ch: char,
    /// `None` for characters without geometry (e.g. generated line breaks).
    pub bbox: Option<BoundingBox>,
    pub font_name: String,
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
}

/// Groups consecutive glyphs into spans: a new span starts at a line break, a font/size/style
/// change, or when the next glyph is not on the current line.
pub fn group_spans(glyphs: impl IntoIterator<Item = Glyph>) -> Vec<TextSpan> {
    let mut spans: Vec<TextSpan> = Vec::new();
    let mut current: Option<TextSpan> = None;

    for glyph in glyphs {
        if matches!(glyph.ch, '\n' | '\r') {
            flush(&mut current, &mut spans);
            continue;
        }
        if let Some(span) = &current {
            let same_style = span.font_name == glyph.font_name
                && (span.font_size - glyph.font_size).abs() < 0.1
                && span.bold == glyph.bold
                && span.italic == glyph.italic;
            let same_line = glyph
                .bbox
                .is_none_or(|b| same_line(&span.bbox, &b, span.font_size));
            if !(same_style && same_line) {
                flush(&mut current, &mut spans);
            }
        }
        match (&mut current, glyph.bbox) {
            (Some(span), bbox) => {
                span.text.push(glyph.ch);
                if let Some(bbox) = bbox.filter(|b| b.width() > 0.0 || !glyph.ch.is_whitespace()) {
                    span.bbox = span.bbox.union(&bbox);
                }
            }
            (None, Some(bbox)) if !glyph.ch.is_whitespace() => {
                current = Some(TextSpan {
                    text: glyph.ch.to_string(),
                    bbox,
                    font_name: glyph.font_name,
                    font_size: glyph.font_size,
                    bold: glyph.bold,
                    italic: glyph.italic,
                });
            }
            // Leading whitespace or glyphs without geometry cannot start a span.
            (None, _) => {}
        }
    }
    flush(&mut current, &mut spans);
    spans
}

/// Two boxes are on the same line when their vertical centers are within ~half a line.
fn same_line(a: &BoundingBox, b: &BoundingBox, font_size: f32) -> bool {
    let center = |r: &BoundingBox| (r.top + r.bottom) / 2.0;
    (center(a) - center(b)).abs() < font_size.max(1.0) * 0.5
}

fn flush(current: &mut Option<TextSpan>, spans: &mut Vec<TextSpan>) {
    if let Some(mut span) = current.take() {
        let trimmed = span.text.trim_end().len();
        span.text.truncate(trimmed);
        if !span.text.is_empty() {
            spans.push(span);
        }
    }
}

/// Font names such as "Helvetica-Bold", "Arial,Bold", "Inter-SemiBold" or "ABCDEF+Roboto-Black".
/// A rectangle in PDF page space (bottom-left origin, unrotated, in points) as the page is
/// displayed and rendered: rotated by `/Rotate` (clockwise degrees) and with a top-left origin.
/// `width`/`height` are the unrotated page size.
pub fn to_display_box(
    (left, bottom, right, top): (f32, f32, f32, f32),
    width: f32,
    height: f32,
    rotation: u32,
) -> BoundingBox {
    let point = |x: f32, y: f32| match rotation % 360 {
        90 => (y, x),
        180 => (width - x, y),
        270 => (height - y, width - x),
        _ => (x, height - y),
    };
    let (ax, ay) = point(left, bottom);
    let (bx, by) = point(right, top);
    BoundingBox {
        left: ax.min(bx),
        top: ay.min(by),
        right: ax.max(bx),
        bottom: ay.max(by),
    }
}

pub fn name_implies_bold(font_name: &str) -> bool {
    let name = font_name.to_ascii_lowercase();
    ["bold", "black", "heavy", "semibold", "demibold"]
        .iter()
        .any(|w| name.contains(w))
}

pub fn name_implies_italic(font_name: &str) -> bool {
    let name = font_name.to_ascii_lowercase();
    name.contains("italic") || name.contains("oblique")
}

/// Converts a PDF date (`D:YYYYMMDDHHmmSSOHH'mm'`, most parts optional) to ISO-8601.
pub fn pdf_date_to_iso(raw: &str) -> Option<String> {
    let s = raw.strip_prefix("D:").unwrap_or(raw);
    let digits = |range: std::ops::Range<usize>, default: &str| -> Option<String> {
        match s.get(range) {
            Some(part) if part.chars().all(|c| c.is_ascii_digit()) => Some(part.to_string()),
            Some(_) => None,
            None => Some(default.to_string()),
        }
    };
    if s.len() < 4 {
        return None;
    }
    let year = digits(0..4, "")?;
    let month = digits(4..6, "01")?;
    let day = digits(6..8, "01")?;
    let hour = digits(8..10, "00")?;
    let minute = digits(10..12, "00")?;
    let second = digits(12..14, "00")?;
    let zone = match s.get(14..) {
        None | Some("") | Some("Z") | Some("Z00'00'") => "Z".to_string(),
        Some(rest) => {
            let sign = rest.chars().next().filter(|c| *c == '+' || *c == '-')?;
            let tz: String = rest[1..].chars().filter(|c| c.is_ascii_digit()).collect();
            let (h, m) = (tz.get(0..2).unwrap_or("00"), tz.get(2..4).unwrap_or("00"));
            format!("{sign}{h}:{m}")
        }
    };
    Some(format!(
        "{year}-{month}-{day}T{hour}:{minute}:{second}{zone}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(ch: char, left: f32, top: f32, size: f32, bold: bool) -> Glyph {
        Glyph {
            ch,
            bbox: Some(BoundingBox {
                left,
                top,
                right: left + size * 0.5,
                bottom: top + size,
            }),
            font_name: if bold { "Helvetica-Bold" } else { "Helvetica" }.into(),
            font_size: size,
            bold,
            italic: false,
        }
    }

    #[test]
    fn groups_by_line_and_style() {
        let mut glyphs: Vec<Glyph> = "Título"
            .chars()
            .enumerate()
            .map(|(i, c)| glyph(c, 72.0 + i as f32 * 9.0, 60.0, 18.0, true))
            .collect();
        glyphs.push(Glyph {
            ch: '\n',
            bbox: None,
            font_name: String::new(),
            font_size: 0.0,
            bold: false,
            italic: false,
        });
        glyphs.extend(
            "Texto aqui"
                .chars()
                .enumerate()
                .map(|(i, c)| glyph(c, 72.0 + i as f32 * 5.5, 90.0, 11.0, false)),
        );
        glyphs.extend(
            "Outra"
                .chars()
                .enumerate()
                .map(|(i, c)| glyph(c, 72.0 + i as f32 * 5.5, 106.0, 11.0, false)),
        );

        let spans = group_spans(glyphs);
        let texts: Vec<_> = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["Título", "Texto aqui", "Outra"]);
        assert!(spans[0].bold && !spans[1].bold);
        assert!(spans[0].bbox.bottom <= spans[1].bbox.top);
    }

    #[test]
    fn rotated_pages_map_boxes_into_the_displayed_page() {
        // A 10×12 pt glyph box near the top-left of an unrotated 612×792 page.
        let rect = (72.0, 708.0, 82.0, 720.0);
        let b = |l, t, r, bo| BoundingBox {
            left: l,
            top: t,
            right: r,
            bottom: bo,
        };
        assert_eq!(
            to_display_box(rect, 612.0, 792.0, 0),
            b(72.0, 72.0, 82.0, 84.0)
        );
        // 90° clockwise: displayed 792×612, the corner goes to the top-right.
        assert_eq!(
            to_display_box(rect, 612.0, 792.0, 90),
            b(708.0, 72.0, 720.0, 82.0)
        );
        assert_eq!(
            to_display_box(rect, 612.0, 792.0, 180),
            b(530.0, 708.0, 540.0, 720.0)
        );
        assert_eq!(
            to_display_box(rect, 612.0, 792.0, 270),
            b(72.0, 530.0, 84.0, 540.0)
        );
        for rotation in [0, 90, 180, 270] {
            let r = to_display_box(rect, 612.0, 792.0, rotation);
            let (w, h) = if rotation % 180 == 0 {
                (612.0, 792.0)
            } else {
                (792.0, 612.0)
            };
            assert!(
                r.left >= 0.0 && r.top >= 0.0 && r.right <= w && r.bottom <= h,
                "{rotation}: {r:?}"
            );
        }
    }

    #[test]
    fn infers_style_from_font_names() {
        assert!(name_implies_bold("Helvetica-Bold") && name_implies_bold("ABCDEF+Inter-SemiBold"));
        assert!(!name_implies_bold("Helvetica") && !name_implies_bold("Times-Roman"));
        assert!(name_implies_italic("Helvetica-Oblique") && name_implies_italic("Arial,Italic"));
    }

    #[test]
    fn parses_pdf_dates() {
        assert_eq!(
            pdf_date_to_iso("D:20260102030405Z").as_deref(),
            Some("2026-01-02T03:04:05Z")
        );
        assert_eq!(
            pdf_date_to_iso("D:20260102030405-03'00'").as_deref(),
            Some("2026-01-02T03:04:05-03:00")
        );
        assert_eq!(
            pdf_date_to_iso("D:2026").as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
        assert_eq!(pdf_date_to_iso("ontem"), None);
    }
}
