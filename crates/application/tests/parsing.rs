//! `PdfDocumentParser` over the fake engine, and the `ParserRegistry`.

use std::{path::PathBuf, sync::Arc};

use nlmx_application::{
    ports::{DocumentParser, DocumentSource, StructureAnalyzer},
    services::parsing::{ParserRegistry, PdfDocumentParser},
};
use nlmx_domain::{
    document::{BoundingBox, DocumentError, DocumentMetadata as PdfMetadata, PageImage, TextSpan},
    document_type::DocumentType,
    ingestion::{Block, BlockKind, PageBox, PageLayout, StructuredDocument},
    parsed::{ContentKind, ParseError},
    source::SourceLocation,
};
use nlmx_testing::{
    FakeDocumentEngine, FakeDocumentParser, FakePage, FakeStructureAnalyzer, ParserSample,
    document_parser_contract, sample_parsed_document,
};

fn span(text: &str) -> TextSpan {
    TextSpan {
        text: text.into(),
        bbox: BoundingBox {
            left: 72.0,
            top: 60.0,
            right: 300.0,
            bottom: 75.0,
        },
        font_name: "Helvetica".into(),
        font_size: 11.0,
        bold: false,
        italic: false,
    }
}

fn text_page(lines: &[&str]) -> FakePage {
    FakePage {
        spans: lines.iter().map(|l| span(l)).collect(),
        images: vec![],
    }
}

fn image_page() -> FakePage {
    FakePage {
        spans: vec![],
        images: vec![PageImage {
            index: 0,
            bbox: BoundingBox {
                left: 0.0,
                top: 0.0,
                right: 612.0,
                bottom: 792.0,
            },
            width_px: 10,
            height_px: 10,
            png: vec![],
        }],
    }
}

/// The first span of each page is a level-1 heading; the others are its paragraphs.
struct HeadingAnalyzer;

impl StructureAnalyzer for HeadingAnalyzer {
    fn version(&self) -> u32 {
        7
    }

    fn analyze(&self, pages: &[PageLayout]) -> StructuredDocument {
        let mut blocks = Vec::new();
        for page in pages {
            let mut heading: Option<String> = None;
            for span in &page.spans {
                let boxes = vec![PageBox {
                    page: page.number,
                    bbox: span.bbox,
                }];
                match &heading {
                    None => {
                        heading = Some(span.text.clone());
                        blocks.push(Block {
                            kind: BlockKind::Heading { level: 1 },
                            text: span.text.clone(),
                            page: page.number,
                            boxes,
                            section_path: vec![],
                        });
                    }
                    Some(title) => blocks.push(Block {
                        kind: BlockKind::Paragraph,
                        text: span.text.clone(),
                        page: page.number,
                        boxes,
                        section_path: vec![title.clone()],
                    }),
                }
            }
        }
        StructuredDocument { blocks }
    }
}

fn parser(engine: FakeDocumentEngine) -> (Arc<FakeDocumentEngine>, PdfDocumentParser) {
    let engine = Arc::new(engine);
    let parser = PdfDocumentParser::new(engine.clone(), Arc::new(HeadingAnalyzer));
    (engine, parser)
}

#[tokio::test]
async fn the_pdf_parser_honours_the_contract() {
    let engine = FakeDocumentEngine::default()
        .with_document(
            "/lib/valid.pdf",
            PdfMetadata {
                page_count: 1,
                ..Default::default()
            },
            vec![text_page(&["Introdução", "O marcador aparece aqui."])],
        )
        .with_open_error("/lib/invalid.pdf", DocumentError::InvalidPdf("lixo".into()));
    let parser = PdfDocumentParser::new(Arc::new(engine), Arc::new(FakeStructureAnalyzer));
    document_parser_contract(
        &parser,
        ParserSample {
            valid: &PathBuf::from("/lib/valid.pdf"),
            marker: "marcador",
            invalid: &PathBuf::from("/lib/invalid.pdf"),
            missing: &PathBuf::from("/lib/missing.pdf"),
        },
    )
    .await;
}

#[tokio::test]
async fn pages_headings_and_metadata_survive() {
    let (engine, parser) = parser(FakeDocumentEngine::default().with_document(
        "/lib/a.pdf",
        PdfMetadata {
            title: Some("Contrato".into()),
            author: Some("Autora".into()),
            created_at: Some("2024-01-02T00:00:00Z".into()),
            pdf_version: Some("1.7".into()),
            page_count: 3,
            ..Default::default()
        },
        vec![
            text_page(&["Escopo", "Primeiro parágrafo.", "Segundo parágrafo."]),
            text_page(&["Prazos", "Cento e oitenta dias."]),
            image_page(),
        ],
    ));
    let parsed = parser
        .parse(&DocumentSource::from_path("/lib/a.pdf"))
        .await
        .unwrap();

    assert_eq!(parser.version(), 7);
    assert_eq!(parsed.document_type(), DocumentType::Pdf);
    let metadata = parsed.metadata();
    assert_eq!(metadata.title.as_deref(), Some("Contrato"));
    assert_eq!(metadata.author.as_deref(), Some("Autora"));
    assert_eq!(metadata.created_at.as_deref(), Some("2024-01-02T00:00:00Z"));
    assert_eq!(metadata.source_version.as_deref(), Some("1.7"));
    assert_eq!(metadata.page_count, Some(3));

    // One summary per page, the image-only page without text.
    let pages: Vec<_> = parsed
        .pages()
        .iter()
        .map(|p| (p.number, p.has_text))
        .collect();
    assert_eq!(pages, [(1, true), (2, true), (3, false)]);
    assert!(!parsed.needs_ocr());

    // One section per heading, with its path, level and starting page.
    let sections = parsed.sections();
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0].title.as_deref(), Some("Escopo"));
    assert_eq!(sections[0].path, ["Escopo"]);
    assert_eq!(sections[0].level, 1);
    assert_eq!(sections[0].blocks.len(), 3);
    assert_eq!(sections[0].location.page_range(), Some((1, 1)));
    assert_eq!(sections[1].path, ["Prazos"]);
    assert_eq!(sections[1].location.page_range(), Some((2, 2)));
    assert_eq!(sections[1].text(), "Prazos\n\nCento e oitenta dias.");

    // The block keeps its kind and the boxes the viewer highlights.
    let first = &sections[0].blocks[0];
    assert_eq!(first.kind, ContentKind::Heading { level: 1 });
    let second = &sections[0].blocks[1];
    assert_eq!(second.kind, ContentKind::Paragraph);
    assert_eq!(second.location.boxes().len(), 1);
    assert!(matches!(second.location, SourceLocation::Pdf { .. }));

    // The engine's document is always released.
    assert_eq!(engine.open_documents(), 0);
}

#[tokio::test]
async fn a_pdf_without_a_text_layer_needs_ocr() {
    let (_, parser) = parser(FakeDocumentEngine::default().with_document(
        "/lib/scan.pdf",
        PdfMetadata {
            page_count: 2,
            ..Default::default()
        },
        vec![image_page(), image_page()],
    ));
    let parsed = parser
        .parse(&DocumentSource::from_path("/lib/scan.pdf"))
        .await
        .unwrap();
    assert!(parsed.needs_ocr());
    assert!(!parsed.has_text());
    assert!(parsed.sections().is_empty());
    assert_eq!(parsed.pages().len(), 2);
}

#[tokio::test]
async fn engine_errors_become_parse_errors() {
    let (_, parser) = parser(
        FakeDocumentEngine::default()
            .with_open_error("/lib/senha.pdf", DocumentError::PasswordRequired)
            .with_open_error("/lib/ruim.pdf", DocumentError::InvalidPdf("segredo".into()))
            .with_open_error("/lib/falha.pdf", DocumentError::Engine("boom".into())),
    );
    let parse = |path: &'static str| {
        let parser = &parser;
        async move { parser.parse(&DocumentSource::from_path(path)).await }
    };
    assert_eq!(parse("/lib/senha.pdf").await, Err(ParseError::Encrypted));
    let invalid = parse("/lib/ruim.pdf").await;
    assert_eq!(invalid, Err(ParseError::Invalid(DocumentType::Pdf)));
    assert!(!invalid.unwrap_err().to_string().contains("segredo"));
    assert!(matches!(
        parse("/lib/falha.pdf").await,
        Err(ParseError::Engine(_))
    ));
    assert_eq!(
        parse("/lib/nao-existe.pdf").await,
        Err(ParseError::NotFound)
    );
}

#[tokio::test]
async fn another_declared_format_is_refused() {
    let (_, parser) = parser(FakeDocumentEngine::default());
    let source = DocumentSource::of_type("/lib/a.pdf", DocumentType::Csv);
    assert_eq!(parser.parse(&source).await, Err(ParseError::Unsupported));
}

fn registry() -> ParserRegistry {
    let csv = FakeDocumentParser::new(DocumentType::Csv).with_document(
        "/in/a.csv",
        sample_parsed_document(DocumentType::Csv, "linha"),
    );
    let text = FakeDocumentParser::new(DocumentType::Text).with_document(
        "/in/b.dat",
        sample_parsed_document(DocumentType::Text, "texto"),
    );
    ParserRegistry::new()
        .with(Arc::new(text))
        .with(Arc::new(csv))
}

#[tokio::test]
async fn the_registry_parses_by_extension_or_declared_format() {
    let registry = registry();
    let csv = registry
        .parse(&DocumentSource::from_path("/in/a.csv"))
        .await
        .unwrap();
    assert_eq!(csv.document_type(), DocumentType::Csv);

    // An unknown extension is fine when the format is declared.
    let text = registry
        .parse(&DocumentSource::of_type("/in/b.dat", DocumentType::Text))
        .await
        .unwrap();
    assert_eq!(text.document_type(), DocumentType::Text);
}

#[tokio::test]
async fn the_registry_reports_unsupported_formats() {
    let registry = registry();
    // Unknown extension and nothing declared.
    assert_eq!(
        registry
            .parse(&DocumentSource::from_path("/in/x.docx"))
            .await,
        Err(ParseError::Unsupported)
    );
    // A known format without a registered parser.
    assert_eq!(
        registry
            .parse(&DocumentSource::from_path("/in/x.epub"))
            .await,
        Err(ParseError::Unsupported)
    );
}

#[test]
fn the_registry_finds_parsers_by_type_and_mime() {
    let registry = registry();
    assert_eq!(
        registry.supported_types(),
        [DocumentType::Text, DocumentType::Csv]
    );
    assert!(registry.parser_for(DocumentType::Csv).is_some());
    assert!(registry.parser_for(DocumentType::Pdf).is_none());
    assert_eq!(
        registry
            .parser_for_mime("text/csv; charset=utf-8")
            .map(|p| p.document_type()),
        Some(DocumentType::Csv)
    );
    assert!(registry.parser_for_mime("application/pdf").is_none());
}

#[tokio::test]
async fn registering_a_format_again_replaces_its_parser() {
    let first = FakeDocumentParser::new(DocumentType::Csv).with_document(
        "/in/a.csv",
        sample_parsed_document(DocumentType::Csv, "antigo"),
    );
    let second = FakeDocumentParser::new(DocumentType::Csv).with_document(
        "/in/a.csv",
        sample_parsed_document(DocumentType::Csv, "novo"),
    );
    let registry = ParserRegistry::new()
        .with(Arc::new(first))
        .with(Arc::new(second));
    assert_eq!(registry.supported_types(), [DocumentType::Csv]);
    let parsed = registry
        .parse(&DocumentSource::from_path("/in/a.csv"))
        .await
        .unwrap();
    assert!(parsed.blocks().any(|b| b.text == "novo"));
}
