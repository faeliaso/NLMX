//! Where a piece of text came from, whatever the format of its document.
//!
//! The RAG, the citations and the storage work with `SourceReference` and `SourceLocation`
//! and never with a parser's details. Only a PDF location can be opened in the viewer
//! (`DocumentType::previewable`); every other format is cited by its label alone.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{
    document_type::DocumentType, ingestion::DocumentId, ingestion::PageBox, vectors::ChunkId,
};

/// Separator between the headings of a section path in labels.
const HEADING_SEPARATOR: &str = " › ";

/// A location that is not valid for its format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationError {
    /// A 1-based position (page, line, row, chapter) is 0.
    ZeroPosition(&'static str),
    /// The end is before the start.
    Reversed(&'static str),
    /// A text range with no characters.
    Empty,
    /// Only one of the two ends of a range is present.
    Incomplete(&'static str),
}

impl fmt::Display for LocationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroPosition(what) => write!(f, "{what} começa em 1"),
            Self::Reversed(what) => write!(f, "intervalo de {what} invertido"),
            Self::Empty => f.write_str("intervalo de texto vazio"),
            Self::Incomplete(what) => write!(f, "intervalo de {what} sem início ou fim"),
        }
    }
}

impl std::error::Error for LocationError {}

/// A position inside a document, in terms of its format.
///
/// Fields are public so a value can be read and matched freely; build one with the checked
/// constructors, and call [`SourceLocation::validate`] on a value that was deserialized.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceLocation {
    /// Pages are 1-based. `boxes` (PDF points, top-left origin) are optional highlights.
    Pdf {
        page_start: u32,
        page_end: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        boxes: Vec<PageBox>,
    },
    /// The headings that enclose the text (outermost first) and, when known, the 1-based
    /// source lines. The section is the last heading.
    Markdown {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        heading_path: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line_start: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line_end: Option<u32>,
    },
    /// The range `[start, end)` in characters (Unicode scalar values) of the decoded text.
    Text { start: u32, end: u32 },
    /// Data rows, 1-based and both included. The header row is not counted.
    Csv { row_start: u32, row_end: u32 },
    /// `chapter_index` is the 1-based position of the chapter in reading order.
    Epub {
        chapter_index: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chapter_title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        section: Option<String>,
    },
}

impl SourceLocation {
    pub fn pdf(page_start: u32, page_end: u32, boxes: Vec<PageBox>) -> Result<Self, LocationError> {
        Self::checked(Self::Pdf {
            page_start,
            page_end,
            boxes,
        })
    }

    /// `lines` is `(first, last)`, both included.
    pub fn markdown(
        heading_path: Vec<String>,
        lines: Option<(u32, u32)>,
    ) -> Result<Self, LocationError> {
        Self::checked(Self::Markdown {
            heading_path,
            line_start: lines.map(|(start, _)| start),
            line_end: lines.map(|(_, end)| end),
        })
    }

    pub fn text(start: u32, end: u32) -> Result<Self, LocationError> {
        Self::checked(Self::Text { start, end })
    }

    pub fn csv(row_start: u32, row_end: u32) -> Result<Self, LocationError> {
        Self::checked(Self::Csv { row_start, row_end })
    }

    pub fn epub(
        chapter_index: u32,
        chapter_title: Option<String>,
        section: Option<String>,
    ) -> Result<Self, LocationError> {
        Self::checked(Self::Epub {
            chapter_index,
            chapter_title,
            section,
        })
    }

    fn checked(location: Self) -> Result<Self, LocationError> {
        location.validate()?;
        Ok(location)
    }

    /// Checks the invariants of the format.
    pub fn validate(&self) -> Result<(), LocationError> {
        match self {
            Self::Pdf {
                page_start,
                page_end,
                ..
            } => range("página", *page_start, *page_end),
            Self::Markdown {
                line_start,
                line_end,
                ..
            } => match (line_start, line_end) {
                (None, None) => Ok(()),
                (Some(start), Some(end)) => range("linhas", *start, *end),
                _ => Err(LocationError::Incomplete("linhas")),
            },
            Self::Text { start, end } => {
                if end < start {
                    Err(LocationError::Reversed("texto"))
                } else if end == start {
                    Err(LocationError::Empty)
                } else {
                    Ok(())
                }
            }
            Self::Csv { row_start, row_end } => range("linhas", *row_start, *row_end),
            Self::Epub { chapter_index, .. } => {
                if *chapter_index == 0 {
                    Err(LocationError::ZeroPosition("capítulo"))
                } else {
                    Ok(())
                }
            }
        }
    }

    /// The format of the document this location points into.
    pub fn document_type(&self) -> DocumentType {
        match self {
            Self::Pdf { .. } => DocumentType::Pdf,
            Self::Markdown { .. } => DocumentType::Markdown,
            Self::Text { .. } => DocumentType::Text,
            Self::Csv { .. } => DocumentType::Csv,
            Self::Epub { .. } => DocumentType::Epub,
        }
    }

    /// Whether the interface can open this location in a viewer.
    pub fn previewable(&self) -> bool {
        self.document_type().previewable()
    }

    /// First and last page, for a paged format.
    pub fn page_range(&self) -> Option<(u32, u32)> {
        match self {
            Self::Pdf {
                page_start,
                page_end,
                ..
            } => Some((*page_start, *page_end)),
            _ => None,
        }
    }

    /// The smallest location of the same format that covers both, or `None` when they are of
    /// different formats (or, for an EPUB, of different chapters). A PDF union keeps both
    /// pages' boxes; Markdown lines survive only when both sides know them and the heading
    /// path is the part the two share; EPUB sections survive only when equal.
    pub fn merge(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (
                Self::Pdf {
                    page_start: a_start,
                    page_end: a_end,
                    boxes: a_boxes,
                },
                Self::Pdf {
                    page_start: b_start,
                    page_end: b_end,
                    boxes: b_boxes,
                },
            ) => {
                let mut boxes = a_boxes.clone();
                for b in b_boxes {
                    if !boxes.contains(b) {
                        boxes.push(*b);
                    }
                }
                Some(Self::Pdf {
                    page_start: *a_start.min(b_start),
                    page_end: *a_end.max(b_end),
                    boxes,
                })
            }
            (
                Self::Markdown {
                    heading_path: a_path,
                    line_start: a_start,
                    line_end: a_end,
                },
                Self::Markdown {
                    heading_path: b_path,
                    line_start: b_start,
                    line_end: b_end,
                },
            ) => {
                let common = a_path
                    .iter()
                    .zip(b_path)
                    .take_while(|(a, b)| a == b)
                    .map(|(a, _)| a.clone())
                    .collect();
                let (line_start, line_end) = match (a_start, a_end, b_start, b_end) {
                    (Some(a_s), Some(a_e), Some(b_s), Some(b_e)) => {
                        (Some(*a_s.min(b_s)), Some(*a_e.max(b_e)))
                    }
                    _ => (None, None),
                };
                Some(Self::Markdown {
                    heading_path: common,
                    line_start,
                    line_end,
                })
            }
            (
                Self::Text {
                    start: a_start,
                    end: a_end,
                },
                Self::Text {
                    start: b_start,
                    end: b_end,
                },
            ) => Some(Self::Text {
                start: *a_start.min(b_start),
                end: *a_end.max(b_end),
            }),
            (
                Self::Csv {
                    row_start: a_start,
                    row_end: a_end,
                },
                Self::Csv {
                    row_start: b_start,
                    row_end: b_end,
                },
            ) => Some(Self::Csv {
                row_start: *a_start.min(b_start),
                row_end: *a_end.max(b_end),
            }),
            (
                Self::Epub {
                    chapter_index: a_index,
                    chapter_title: a_title,
                    section: a_section,
                },
                Self::Epub {
                    chapter_index: b_index,
                    chapter_title: b_title,
                    section: b_section,
                },
            ) if a_index == b_index => Some(Self::Epub {
                chapter_index: *a_index,
                chapter_title: a_title.clone().or_else(|| b_title.clone()),
                section: if a_section == b_section {
                    a_section.clone()
                } else {
                    None
                },
            }),
            _ => None,
        }
    }

    /// The part of a location that a following chunk repeats as overlap: for a PDF only its
    /// last page, with the boxes on it; any other location is returned whole.
    pub fn trailing(&self) -> Self {
        match self {
            Self::Pdf {
                page_end, boxes, ..
            } => Self::Pdf {
                page_start: *page_end,
                page_end: *page_end,
                boxes: boxes
                    .iter()
                    .filter(|b| b.page == *page_end)
                    .copied()
                    .collect(),
            },
            other => other.clone(),
        }
    }

    /// Highlight boxes; empty for formats without coordinates.
    pub fn boxes(&self) -> &[PageBox] {
        match self {
            Self::Pdf { boxes, .. } => boxes,
            _ => &[],
        }
    }

    /// Short human description, e.g. "p. 4", "Introdução › Instalação", "linhas 10–40".
    pub fn label(&self) -> String {
        match self {
            Self::Pdf {
                page_start,
                page_end,
                ..
            } if page_start == page_end => format!("p. {page_start}"),
            Self::Pdf {
                page_start,
                page_end,
                ..
            } => format!("pp. {page_start}–{page_end}"),
            Self::Markdown {
                heading_path,
                line_start,
                line_end,
            } => {
                let mut parts = Vec::new();
                if !heading_path.is_empty() {
                    parts.push(heading_path.join(HEADING_SEPARATOR));
                }
                if let (Some(start), Some(end)) = (line_start, line_end) {
                    parts.push(lines_label(*start, *end));
                }
                if parts.is_empty() {
                    "documento".to_string()
                } else {
                    parts.join(", ")
                }
            }
            Self::Text { start, end } => format!("caracteres {start}–{end}"),
            Self::Csv { row_start, row_end } => lines_label(*row_start, *row_end),
            Self::Epub {
                chapter_index,
                chapter_title,
                section,
            } => {
                let mut label = format!("cap. {chapter_index}");
                if let Some(title) = chapter_title.as_deref().filter(|t| !t.is_empty()) {
                    label.push_str(" — ");
                    label.push_str(title);
                }
                if let Some(section) = section.as_deref().filter(|s| !s.is_empty()) {
                    label.push_str(", ");
                    label.push_str(section);
                }
                label
            }
        }
    }
}

fn range(what: &'static str, start: u32, end: u32) -> Result<(), LocationError> {
    if start == 0 || end == 0 {
        Err(LocationError::ZeroPosition(what))
    } else if end < start {
        Err(LocationError::Reversed(what))
    } else {
        Ok(())
    }
}

fn lines_label(start: u32, end: u32) -> String {
    if start == end {
        format!("linha {start}")
    } else {
        format!("linhas {start}–{end}")
    }
}

/// A cited source: a place in a document, independent of how the document was parsed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceReference {
    pub document_id: DocumentId,
    pub document_title: String,
    /// The chunk the text came from, when it still exists.
    pub chunk_id: Option<ChunkId>,
    pub location: SourceLocation,
    /// Headings that enclose the text, outermost first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub section_path: Vec<String>,
}

impl SourceReference {
    /// The format of the document, always the one of the location (they cannot disagree).
    pub fn document_type(&self) -> DocumentType {
        self.location.document_type()
    }

    pub fn previewable(&self) -> bool {
        self.location.previewable()
    }

    /// "Title, p. 4" — the title followed by the location.
    pub fn label(&self) -> String {
        if self.document_title.is_empty() {
            self.location.label()
        } else {
            format!("{}, {}", self.document_title, self.location.label())
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::document::BoundingBox;

    fn sample_box(page: u32) -> PageBox {
        PageBox {
            page,
            bbox: BoundingBox {
                left: 10.0,
                top: 20.0,
                right: 110.0,
                bottom: 40.0,
            },
        }
    }

    fn one_of_each() -> Vec<SourceLocation> {
        vec![
            SourceLocation::pdf(3, 4, vec![sample_box(3)]).unwrap(),
            SourceLocation::pdf(7, 7, vec![]).unwrap(),
            SourceLocation::markdown(vec!["Guia".into(), "Instalação".into()], Some((10, 24)))
                .unwrap(),
            SourceLocation::markdown(vec![], None).unwrap(),
            SourceLocation::text(120, 480).unwrap(),
            SourceLocation::csv(2, 40).unwrap(),
            SourceLocation::epub(3, Some("A viagem".into()), Some("Partida".into())).unwrap(),
            SourceLocation::epub(1, None, None).unwrap(),
        ]
    }

    #[test]
    fn constructors_accept_valid_locations() {
        for location in one_of_each() {
            assert_eq!(location.validate(), Ok(()));
        }
    }

    #[test]
    fn constructors_reject_invalid_locations() {
        assert_eq!(
            SourceLocation::pdf(0, 1, vec![]),
            Err(LocationError::ZeroPosition("página"))
        );
        assert_eq!(
            SourceLocation::pdf(5, 4, vec![]),
            Err(LocationError::Reversed("página"))
        );
        assert_eq!(
            SourceLocation::markdown(vec![], Some((0, 3))),
            Err(LocationError::ZeroPosition("linhas"))
        );
        assert_eq!(
            SourceLocation::markdown(vec![], Some((9, 3))),
            Err(LocationError::Reversed("linhas"))
        );
        assert_eq!(SourceLocation::text(5, 5), Err(LocationError::Empty));
        assert_eq!(
            SourceLocation::text(9, 5),
            Err(LocationError::Reversed("texto"))
        );
        assert_eq!(
            SourceLocation::csv(0, 0),
            Err(LocationError::ZeroPosition("linhas"))
        );
        assert_eq!(
            SourceLocation::csv(3, 2),
            Err(LocationError::Reversed("linhas"))
        );
        assert_eq!(
            SourceLocation::epub(0, None, None),
            Err(LocationError::ZeroPosition("capítulo"))
        );
    }

    #[test]
    fn an_incomplete_line_range_is_invalid() {
        let location = SourceLocation::Markdown {
            heading_path: vec![],
            line_start: Some(1),
            line_end: None,
        };
        assert_eq!(
            location.validate(),
            Err(LocationError::Incomplete("linhas"))
        );
    }

    #[test]
    fn each_location_knows_its_document_type() {
        let types: Vec<_> = one_of_each().iter().map(|l| l.document_type()).collect();
        assert_eq!(
            types,
            [
                DocumentType::Pdf,
                DocumentType::Pdf,
                DocumentType::Markdown,
                DocumentType::Markdown,
                DocumentType::Text,
                DocumentType::Csv,
                DocumentType::Epub,
                DocumentType::Epub,
            ]
        );
    }

    #[test]
    fn only_a_pdf_location_is_previewable() {
        for location in one_of_each() {
            assert_eq!(
                location.previewable(),
                location.document_type() == DocumentType::Pdf
            );
        }
    }

    #[test]
    fn pages_and_boxes_exist_only_for_pdf() {
        let pdf = SourceLocation::pdf(3, 4, vec![sample_box(3)]).unwrap();
        assert_eq!(pdf.page_range(), Some((3, 4)));
        assert_eq!(pdf.boxes(), [sample_box(3)]);

        for location in one_of_each().into_iter().filter(|l| !l.previewable()) {
            assert_eq!(location.page_range(), None);
            assert!(location.boxes().is_empty());
        }
        assert!(
            SourceLocation::pdf(7, 7, vec![])
                .unwrap()
                .boxes()
                .is_empty()
        );
    }

    #[test]
    fn labels_describe_each_location() {
        let labels: Vec<_> = one_of_each().iter().map(|l| l.label()).collect();
        assert_eq!(
            labels,
            [
                "pp. 3–4",
                "p. 7",
                "Guia › Instalação, linhas 10–24",
                "documento",
                "caracteres 120–480",
                "linhas 2–40",
                "cap. 3 — A viagem, Partida",
                "cap. 1",
            ]
        );
        assert_eq!(SourceLocation::csv(5, 5).unwrap().label(), "linha 5");
        assert_eq!(
            SourceLocation::markdown(vec!["Intro".into()], None)
                .unwrap()
                .label(),
            "Intro"
        );
    }

    #[test]
    fn merging_covers_both_locations() {
        let pdf = |a, b, boxes| SourceLocation::pdf(a, b, boxes).unwrap();
        let merged = pdf(3, 4, vec![sample_box(3)])
            .merge(&pdf(4, 6, vec![sample_box(3), sample_box(5)]))
            .unwrap();
        assert_eq!(
            merged,
            pdf(3, 6, vec![sample_box(3), sample_box(5)]),
            "boxes are kept once"
        );

        let md = |path: &[&str], lines| {
            SourceLocation::markdown(path.iter().map(|s| s.to_string()).collect(), lines).unwrap()
        };
        assert_eq!(
            md(&["Guia", "A"], Some((3, 9)))
                .merge(&md(&["Guia", "B"], Some((12, 20))))
                .unwrap(),
            md(&["Guia"], Some((3, 20)))
        );
        assert_eq!(
            md(&["Guia"], Some((3, 9)))
                .merge(&md(&["Guia"], None))
                .unwrap(),
            md(&["Guia"], None),
            "unknown lines on one side are unknown on the union"
        );

        let text = SourceLocation::text(10, 20).unwrap();
        assert_eq!(
            text.merge(&SourceLocation::text(5, 12).unwrap()).unwrap(),
            SourceLocation::text(5, 20).unwrap()
        );
        assert_eq!(
            SourceLocation::csv(4, 9)
                .unwrap()
                .merge(&SourceLocation::csv(2, 5).unwrap())
                .unwrap(),
            SourceLocation::csv(2, 9).unwrap()
        );

        let epub = |chapter, section: Option<&str>| {
            SourceLocation::epub(chapter, Some("Cap".into()), section.map(String::from)).unwrap()
        };
        assert_eq!(
            epub(2, Some("A")).merge(&epub(2, Some("A"))).unwrap(),
            epub(2, Some("A"))
        );
        assert_eq!(
            epub(2, Some("A")).merge(&epub(2, Some("B"))).unwrap(),
            epub(2, None)
        );
        assert_eq!(epub(2, None).merge(&epub(3, None)), None);
    }

    #[test]
    fn locations_of_different_formats_do_not_merge() {
        let all = one_of_each();
        for a in &all {
            for b in &all {
                if a.document_type() != b.document_type() {
                    assert_eq!(a.merge(b), None);
                }
            }
        }
    }

    #[test]
    fn a_merge_is_valid_and_independent_of_order() {
        let all = one_of_each();
        for a in &all {
            for b in &all {
                let Some(ab) = a.merge(b) else { continue };
                assert_eq!(ab.validate(), Ok(()), "{a:?} + {b:?}");
                assert_eq!(ab.document_type(), a.document_type());
                // The covered range does not depend on the order (boxes may).
                let ba = b.merge(a).expect("symmetric");
                assert_eq!(ab.page_range(), ba.page_range());
                assert_eq!(ab.label().is_empty(), ba.label().is_empty());
            }
            assert_eq!(
                a.merge(a).as_ref(),
                Some(a),
                "merging with itself is a no-op"
            );
        }
    }

    #[test]
    fn the_trailing_part_of_a_pdf_location_is_its_last_page() {
        let pdf = SourceLocation::pdf(3, 5, vec![sample_box(3), sample_box(5)]).unwrap();
        assert_eq!(
            pdf.trailing(),
            SourceLocation::pdf(5, 5, vec![sample_box(5)]).unwrap()
        );
        for location in one_of_each().into_iter().filter(|l| !l.previewable()) {
            assert_eq!(location.trailing(), location);
        }
    }

    #[test]
    fn locations_round_trip_through_json() {
        for location in one_of_each() {
            let json = serde_json::to_string(&location).unwrap();
            let back: SourceLocation = serde_json::from_str(&json).unwrap();
            assert_eq!(back, location, "{json}");
        }
    }

    #[test]
    fn the_json_shape_is_stable() {
        let pdf = SourceLocation::pdf(3, 4, vec![sample_box(3)]).unwrap();
        assert_eq!(
            serde_json::to_value(&pdf).unwrap(),
            json!({
                "kind": "pdf",
                "page_start": 3,
                "page_end": 4,
                "boxes": [{
                    "page": 3,
                    "bbox": {"left": 10.0, "top": 20.0, "right": 110.0, "bottom": 40.0}
                }]
            })
        );
        assert_eq!(
            serde_json::to_value(SourceLocation::pdf(7, 7, vec![]).unwrap()).unwrap(),
            json!({"kind": "pdf", "page_start": 7, "page_end": 7})
        );
        assert_eq!(
            serde_json::to_value(SourceLocation::text(1, 9).unwrap()).unwrap(),
            json!({"kind": "text", "start": 1, "end": 9})
        );
        assert_eq!(
            serde_json::to_value(SourceLocation::csv(2, 40).unwrap()).unwrap(),
            json!({"kind": "csv", "row_start": 2, "row_end": 40})
        );
        assert_eq!(
            serde_json::to_value(
                SourceLocation::markdown(vec!["Guia".into()], Some((1, 2))).unwrap()
            )
            .unwrap(),
            json!({
                "kind": "markdown",
                "heading_path": ["Guia"],
                "line_start": 1,
                "line_end": 2
            })
        );
        assert_eq!(
            serde_json::to_value(SourceLocation::epub(2, Some("Um".into()), None).unwrap())
                .unwrap(),
            json!({"kind": "epub", "chapter_index": 2, "chapter_title": "Um"})
        );
    }

    #[test]
    fn optional_fields_may_be_missing_when_reading() {
        let pdf: SourceLocation =
            serde_json::from_value(json!({"kind": "pdf", "page_start": 1, "page_end": 2})).unwrap();
        assert_eq!(pdf, SourceLocation::pdf(1, 2, vec![]).unwrap());
        let markdown: SourceLocation = serde_json::from_value(json!({"kind": "markdown"})).unwrap();
        assert_eq!(markdown, SourceLocation::markdown(vec![], None).unwrap());
    }

    #[test]
    fn unknown_or_incomplete_json_is_rejected() {
        assert!(serde_json::from_value::<SourceLocation>(json!({"kind": "docx"})).is_err());
        assert!(serde_json::from_value::<SourceLocation>(json!({"page_start": 1})).is_err());
        assert!(
            serde_json::from_value::<SourceLocation>(json!({"kind": "csv", "row_start": 1}))
                .is_err()
        );
    }

    #[test]
    fn a_deserialized_invalid_location_fails_validation() {
        let location: SourceLocation =
            serde_json::from_value(json!({"kind": "pdf", "page_start": 0, "page_end": 0})).unwrap();
        assert_eq!(
            location.validate(),
            Err(LocationError::ZeroPosition("página"))
        );
    }

    #[test]
    fn a_reference_takes_its_type_and_preview_from_the_location() {
        let pdf = SourceReference {
            document_id: 1,
            document_title: "Relatório".into(),
            chunk_id: Some(10),
            location: SourceLocation::pdf(4, 4, vec![]).unwrap(),
            section_path: vec!["Resultados".into()],
        };
        assert_eq!(pdf.document_type(), DocumentType::Pdf);
        assert!(pdf.previewable());
        assert_eq!(pdf.label(), "Relatório, p. 4");

        let csv = SourceReference {
            location: SourceLocation::csv(2, 9).unwrap(),
            document_title: "Vendas".into(),
            ..pdf.clone()
        };
        assert_eq!(csv.document_type(), DocumentType::Csv);
        assert!(!csv.previewable());
        assert_eq!(csv.label(), "Vendas, linhas 2–9");

        let untitled = SourceReference {
            document_title: String::new(),
            ..csv
        };
        assert_eq!(untitled.label(), "linhas 2–9");
    }

    #[test]
    fn a_reference_round_trips_through_json() {
        let reference = SourceReference {
            document_id: 3,
            document_title: "Manual".into(),
            chunk_id: None,
            location: SourceLocation::markdown(vec!["Guia".into()], None).unwrap(),
            section_path: vec!["Guia".into()],
        };
        let json = serde_json::to_string(&reference).unwrap();
        assert_eq!(
            serde_json::from_str::<SourceReference>(&json).unwrap(),
            reference
        );
    }
}
