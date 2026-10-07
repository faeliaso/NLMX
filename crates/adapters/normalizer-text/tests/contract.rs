//! `TextNormalizer` against the shared normalizer contract.

use nlmx_domain::{
    document_type::DocumentType,
    parsed::{
        ContentBlock, ContentKind, DocumentMetadata, DocumentSection, ParsedDocument, RecordField,
    },
    source::SourceLocation,
};
use nlmx_normalizer_text::TextNormalizer;
use nlmx_testing::{document_normalizer_contract, sample_structured_document};

fn dirty_markdown() -> ParsedDocument {
    let block = |kind, text: &str, line| ContentBlock {
        kind,
        text: text.into(),
        location: SourceLocation::markdown(vec!["Guia".into()], Some((line, line + 1))).unwrap(),
    };
    let blocks = vec![
        block(
            ContentKind::Heading { level: 1 },
            "Gu\u{ad}ia\u{a0}rápido",
            1,
        ),
        block(
            ContentKind::Paragraph,
            "Joa\u{303}o  \u{200b}usa  o ﬁltro\n\nnovo.",
            3,
        ),
        block(
            ContentKind::CodeBlock {
                language: Some("rust".into()),
            },
            "fn main() {\r\n    let x = 1;\r\n}",
            5,
        ),
        block(ContentKind::Paragraph, "\u{200b}\u{feff}", 9),
    ];
    ParsedDocument::new(
        DocumentType::Markdown,
        DocumentMetadata {
            title: Some("  Guia\u{a0}rápido ".into()),
            ..Default::default()
        },
        vec![],
        vec![DocumentSection::new(Some("Guia".into()), 1, vec!["Guia".into()], blocks).unwrap()],
        vec![],
    )
    .unwrap()
}

fn dirty_csv() -> ParsedDocument {
    let record = |row: u32, name: &str, value: &str| ContentBlock {
        kind: ContentKind::Record {
            fields: vec![RecordField {
                name: name.into(),
                value: value.into(),
            }],
        },
        text: format!("Registro {row}:\n{name}:\u{a0}{value}\n\n"),
        location: SourceLocation::csv(row, row).unwrap(),
    };
    ParsedDocument::new(
        DocumentType::Csv,
        DocumentMetadata::default(),
        vec![],
        vec![
            DocumentSection::new(
                None,
                0,
                vec![],
                vec![
                    record(1, "Nome", "Joa\u{303}o"),
                    record(2, "Nome", "Maria  "),
                ],
            )
            .unwrap(),
        ],
        vec![],
    )
    .unwrap()
}

#[test]
fn the_text_normalizer_honours_the_contract() {
    let mut samples: Vec<_> = DocumentType::ALL
        .into_iter()
        .map(|kind| sample_structured_document(kind, 3))
        .collect();
    samples.push(dirty_markdown());
    samples.push(dirty_csv());
    document_normalizer_contract(&TextNormalizer, &samples);
}
