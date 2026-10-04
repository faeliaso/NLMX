//! The five parsers behind one `ParserRegistry`, the PDF one on real PDFium and the fixtures
//! of every format.

mod support;

use std::sync::Arc;

use nlmx_application::ports::StructureAnalyzer;
use nlmx_application::{
    ports::{DocumentEngine, DocumentSource},
    services::parsing::{ParserRegistry, PdfDocumentParser},
};
use nlmx_domain::{
    document_type::DocumentType, ingestion::PageLayout, parsed::ParseError, source::SourceLocation,
};
use nlmx_parser_epub::EpubDocumentParser;
use nlmx_parser_text::{CsvDocumentParser, MarkdownDocumentParser, TextDocumentParser};
use nlmx_pdf_pdfium::PdfiumDocumentEngine;
use nlmx_structure_heuristic::HeuristicStructureAnalyzer;
use support::{fixture, root};

/// PDFium can be initialized once per process, so every test shares one engine.
fn engine() -> Arc<PdfiumDocumentEngine> {
    static ENGINE: std::sync::OnceLock<Arc<PdfiumDocumentEngine>> = std::sync::OnceLock::new();
    ENGINE
        .get_or_init(|| {
            Arc::new(PdfiumDocumentEngine::from_default_location().expect("make bootstrap"))
        })
        .clone()
}

fn registry() -> ParserRegistry {
    ParserRegistry::new()
        .with(Arc::new(PdfDocumentParser::new(
            engine(),
            Arc::new(HeuristicStructureAnalyzer),
        )))
        .with(Arc::new(MarkdownDocumentParser))
        .with(Arc::new(TextDocumentParser))
        .with(Arc::new(CsvDocumentParser))
        .with(Arc::new(EpubDocumentParser::default()))
}

fn text_fixture(name: &str) -> std::path::PathBuf {
    root()
        .join("crates/adapters/parser-text/tests/fixtures")
        .join(name)
}

fn epub_fixture(name: &str) -> std::path::PathBuf {
    root()
        .join("crates/adapters/parser-epub/tests/fixtures")
        .join(name)
}

/// What `DocumentIngestion` reads today: the engine's pages through the structure analyzer.
async fn direct_layouts(engine: &PdfiumDocumentEngine, name: &str) -> Vec<PageLayout> {
    let handle = engine.open(&fixture(name)).await.unwrap();
    let metadata = engine.metadata(handle).await.unwrap();
    let mut pages = Vec::new();
    for number in 1..=metadata.page_count {
        let info = engine.page_info(handle, number).await.unwrap();
        let spans = if info.has_text {
            engine.text_spans(handle, number).await.unwrap()
        } else {
            Vec::new()
        };
        pages.push(PageLayout {
            number,
            width: info.width,
            height: info.height,
            has_text: info.has_text,
            char_count: info.char_count,
            spans,
        });
    }
    engine.close(handle).await.unwrap();
    pages
}

#[tokio::test]
async fn the_pdf_parser_matches_what_the_pipeline_reads_today() {
    let engine = engine();
    let parser = PdfDocumentParser::new(engine.clone(), Arc::new(HeuristicStructureAnalyzer));
    for name in [
        "report.pdf",
        "text.pdf",
        "rotated.pdf",
        "unicode.pdf",
        "mixed.pdf",
    ] {
        let layouts = direct_layouts(&engine, name).await;
        let structured = HeuristicStructureAnalyzer.analyze(&layouts);
        let parsed = nlmx_application::ports::DocumentParser::parse(
            &parser,
            &DocumentSource::from_path(fixture(name)),
        )
        .await
        .unwrap_or_else(|e| panic!("{name}: {e}"));

        assert_eq!(parsed.document_type(), DocumentType::Pdf, "{name}");
        assert_eq!(parsed.pages().len(), layouts.len(), "{name}");
        for (page, layout) in parsed.pages().iter().zip(&layouts) {
            assert_eq!(
                (page.number, page.has_text, page.char_count),
                (layout.number, layout.has_text, layout.char_count),
                "{name}"
            );
            assert_eq!(
                (page.width, page.height),
                (layout.width, layout.height),
                "{name}"
            );
        }
        // Same blocks, same text, same page and boxes (what the viewer highlights).
        let blocks: Vec<_> = parsed.blocks().collect();
        assert_eq!(blocks.len(), structured.blocks.len(), "{name}");
        for (block, original) in blocks.iter().zip(&structured.blocks) {
            assert_eq!(block.text, original.text, "{name}");
            let SourceLocation::Pdf {
                page_start, boxes, ..
            } = &block.location
            else {
                panic!("{name}: a PDF block must have a PDF location");
            };
            assert_eq!(*page_start, original.page, "{name}");
            assert_eq!(boxes, &original.boxes, "{name}");
        }
        assert_eq!(parsed.validate(), Ok(()));
        assert!(parsed.has_text(), "{name}");
        assert!(!parsed.needs_ocr(), "{name}");
    }
}

#[tokio::test]
async fn pdfs_that_cannot_be_read_as_text_are_told_apart() {
    let registry = registry();

    let scanned = registry
        .parse(&DocumentSource::from_path(fixture("scanned.pdf")))
        .await
        .unwrap();
    assert!(scanned.needs_ocr());
    assert!(!scanned.has_text());

    assert_eq!(
        registry
            .parse(&DocumentSource::from_path(fixture("encrypted.pdf")))
            .await,
        Err(ParseError::Encrypted)
    );
    assert_eq!(
        registry
            .parse(&DocumentSource::from_path(fixture("corrupt.pdf")))
            .await,
        Err(ParseError::Invalid(DocumentType::Pdf))
    );
}

#[tokio::test]
async fn every_format_is_parsed_with_its_own_kind_of_location() {
    let registry = registry();
    assert_eq!(registry.supported_types(), DocumentType::ALL);

    let cases = [
        (fixture("report.pdf"), DocumentType::Pdf),
        (text_fixture("guia.md"), DocumentType::Markdown),
        (text_fixture("notas.txt"), DocumentType::Text),
        (text_fixture("vendas.csv"), DocumentType::Csv),
        (epub_fixture("livro.epub"), DocumentType::Epub),
    ];
    for (path, kind) in cases {
        let parsed = registry
            .parse(&DocumentSource::from_path(&path))
            .await
            .unwrap_or_else(|e| panic!("{kind}: {e}"));
        assert_eq!(parsed.document_type(), kind);
        assert!(parsed.has_text(), "{kind}");
        assert_eq!(parsed.validate(), Ok(()), "{kind}");
        for block in parsed.blocks() {
            assert_eq!(block.location.document_type(), kind);
            // Only a PDF location can be opened in the viewer.
            assert_eq!(block.location.previewable(), kind == DocumentType::Pdf);
            assert!(!block.location.label().is_empty());
        }
        // Only a paged format has pages.
        assert_eq!(!parsed.pages().is_empty(), kind.is_paged(), "{kind}");
    }
}

#[tokio::test]
async fn the_registry_refuses_unknown_files() {
    let registry = registry();
    assert_eq!(
        registry
            .parse(&DocumentSource::from_path(text_fixture("guia.docx")))
            .await,
        Err(ParseError::Unsupported)
    );
}

#[tokio::test]
async fn a_csv_goes_from_the_file_to_semantic_chunks() {
    use nlmx_chunker_structural::{HeuristicTokenCounter, RecordChunker};
    use nlmx_domain::{ingestion::ChunkPolicy, parsed::ChunkContext};
    use nlmx_parser_text::CsvStream;

    let context = ChunkContext {
        document_id: 1,
        document_title: None,
        file_name: Some("pessoas.csv".into()),
        language: None,
    };

    let path = text_fixture("pessoas.csv");
    let expected_text = "Arquivo: pessoas.csv\n\
        Colunas: Nome, Idade, Cidade, Profissão\n\
        Linhas 1–2\n\n\
        Registro 1:\nNome: João\nIdade: 32\nCidade: Fortaleza\nProfissão: Engenheiro\n\n\
        Registro 2:\nNome: Maria\nIdade: 28\nCidade: Recife\nProfissão: Designer";

    // Incrementally: the stream feeds the chunker without a ParsedDocument in between.
    let stream = CsvStream::open(&path).unwrap();
    let dataset = stream.dataset().clone();
    assert!(dataset.has_header);
    let chunks: Vec<_> = RecordChunker
        .chunk(
            &context,
            &dataset,
            stream.map(Result::unwrap),
            &ChunkPolicy::default(),
            &HeuristicTokenCounter,
        )
        .collect();
    assert_eq!(chunks.len(), 1);
    let chunk = &chunks[0];
    assert_eq!(chunk.text, expected_text);
    assert_eq!(chunk.location, SourceLocation::csv(1, 2).unwrap());
    assert_eq!(chunk.metadata.file_name.as_deref(), Some("pessoas.csv"));
    assert_eq!(
        chunk.metadata.columns,
        ["Nome", "Idade", "Cidade", "Profissão"]
    );
    assert!(!chunk.location.previewable());

    // The same through the registry: the ParsedDocument carries the dataset and the records.
    let parsed = registry()
        .parse(&DocumentSource::from_path(&path))
        .await
        .unwrap();
    let dataset = parsed.metadata().dataset.clone().expect("dataset metadata");
    assert_eq!(dataset.row_count, Some(2));
    let via_registry: Vec<_> = RecordChunker
        .chunk(
            &context,
            &dataset,
            parsed.blocks().cloned(),
            &ChunkPolicy::default(),
            &HeuristicTokenCounter,
        )
        .collect();
    assert_eq!(via_registry, chunks);
}
