//! `ContentPipeline`: parse → normalize → chunk, each stage behind its own port.

use std::sync::{Arc, Mutex};

use nlmx_application::{
    ports::{
        BoxFuture, DocumentChunker, DocumentNormalizer, DocumentParser, DocumentSource,
        TokenCounter,
    },
    services::{parsing::ParserRegistry, pipeline::ContentPipeline},
};
use nlmx_domain::{
    document_type::DocumentType,
    ingestion::ChunkPolicy,
    parsed::{
        ChunkContext, ContentKind, DocumentChunk, DocumentMetadata, PageSummary, ParseError,
        ParsedDocument,
    },
};
use nlmx_testing::{
    FakeDocumentChunker, FakeDocumentNormalizer, FakeDocumentParser, WordTokenCounter,
    sample_structured_document,
};

type Log = Arc<Mutex<Vec<&'static str>>>;

/// Records when each stage runs.
struct Logged<T> {
    name: &'static str,
    log: Log,
    inner: T,
}

impl DocumentParser for Logged<FakeDocumentParser> {
    fn document_type(&self) -> DocumentType {
        self.inner.document_type()
    }
    fn version(&self) -> u32 {
        self.inner.version()
    }
    fn parse<'a>(
        &'a self,
        source: &'a DocumentSource,
    ) -> BoxFuture<'a, Result<ParsedDocument, ParseError>> {
        self.log.lock().unwrap().push(self.name);
        self.inner.parse(source)
    }
}

impl DocumentNormalizer for Logged<Box<dyn DocumentNormalizer>> {
    fn version(&self) -> u32 {
        self.inner.version()
    }
    fn normalize(&self, document: ParsedDocument) -> ParsedDocument {
        self.log.lock().unwrap().push(self.name);
        self.inner.normalize(document)
    }
}

impl DocumentChunker for Logged<FakeDocumentChunker> {
    fn version(&self) -> u32 {
        self.inner.version()
    }
    fn chunk(
        &self,
        document: &ParsedDocument,
        context: &ChunkContext,
        policy: &ChunkPolicy,
        tokens: &dyn TokenCounter,
    ) -> Vec<DocumentChunk> {
        self.log.lock().unwrap().push(self.name);
        self.inner.chunk(document, context, policy, tokens)
    }
}

/// Upper-cases the text of every block: a stand-in for a normalizer that changes text only.
struct Shout;

impl DocumentNormalizer for Shout {
    fn version(&self) -> u32 {
        3
    }
    fn normalize(&self, document: ParsedDocument) -> ParsedDocument {
        let (kind, metadata, pages, mut sections, warnings) = document.into_parts();
        for section in &mut sections {
            for block in &mut section.blocks {
                block.text = block.text.to_uppercase();
            }
        }
        ParsedDocument::new(kind, metadata, pages, sections, warnings).unwrap()
    }
}

struct Setup {
    pipeline: ContentPipeline,
    log: Log,
}

fn setup(
    kind: DocumentType,
    path: &str,
    result: Result<ParsedDocument, ParseError>,
    normalizer: Box<dyn DocumentNormalizer>,
) -> Setup {
    let log: Log = Log::default();
    let parser = match result {
        Ok(document) => FakeDocumentParser::new(kind).with_document(path, document),
        Err(error) => FakeDocumentParser::new(kind).with_error(path, error),
    };
    let pipeline = ContentPipeline {
        parsers: ParserRegistry::new().with(Arc::new(Logged {
            name: "parse",
            log: log.clone(),
            inner: parser,
        })),
        normalizer: Arc::new(Logged {
            name: "normalize",
            log: log.clone(),
            inner: normalizer,
        }),
        chunker: Arc::new(Logged {
            name: "chunk",
            log: log.clone(),
            inner: FakeDocumentChunker,
        }),
        tokens: Arc::new(WordTokenCounter),
        policy: ChunkPolicy::default(),
    };
    Setup { pipeline, log }
}

fn ext(kind: DocumentType) -> &'static str {
    kind.extensions()[0]
}

#[tokio::test]
async fn the_stages_run_in_order_and_chunks_keep_their_provenance() {
    for kind in DocumentType::ALL {
        let path = format!("/in/arquivo.{}", ext(kind));
        let document = sample_structured_document(kind, 3);
        let expected: Vec<_> = document
            .blocks()
            .filter(|b| !matches!(b.kind, ContentKind::Heading { .. }))
            .map(|b| b.location.clone())
            .collect();
        let s = setup(
            kind,
            &path,
            Ok(document),
            Box::new(FakeDocumentNormalizer::default()),
        );

        let processed = s
            .pipeline
            .run(&DocumentSource::from_path(&path), 42, Some("arquivo.ext"))
            .await
            .unwrap_or_else(|e| panic!("{kind}: {e}"));

        assert_eq!(
            *s.log.lock().unwrap(),
            ["parse", "normalize", "chunk"],
            "{kind}"
        );
        assert_eq!(processed.document_type, kind);
        assert!(!processed.needs_ocr);
        assert_eq!(processed.pages.is_empty(), !kind.is_paged(), "{kind}");
        let locations: Vec<_> = processed
            .chunks
            .iter()
            .map(|c| c.location.clone())
            .collect();
        assert_eq!(locations, expected, "{kind}: the source location survives");
        for chunk in &processed.chunks {
            assert_eq!(chunk.document_id, 42);
            assert_eq!(chunk.metadata.file_name.as_deref(), Some("arquivo.ext"));
            assert_eq!(chunk.metadata.document_type, kind);
            assert_eq!(chunk.chunk_id, None);
            assert!(chunk.token_count.is_some());
        }
        // Each stage's version is reported.
        assert_eq!(processed.versions.parser, 97);
        assert_eq!(processed.versions.normalizer, 95);
        assert_eq!(processed.versions.chunker, 94);
    }
}

#[tokio::test]
async fn the_chunker_sees_the_normalized_document() {
    let kind = DocumentType::Markdown;
    let s = setup(
        kind,
        "/in/a.md",
        Ok(sample_structured_document(kind, 2)),
        Box::new(Shout),
    );
    let processed = s
        .pipeline
        .run(&DocumentSource::from_path("/in/a.md"), 1, None)
        .await
        .unwrap();
    assert!(!processed.chunks.is_empty());
    for chunk in &processed.chunks {
        assert_eq!(chunk.text, chunk.text.to_uppercase());
    }
    assert_eq!(processed.versions.normalizer, 3);
}

#[tokio::test]
async fn a_failing_parser_stops_the_pipeline() {
    let s = setup(
        DocumentType::Csv,
        "/in/a.csv",
        Err(ParseError::Invalid(DocumentType::Csv)),
        Box::new(FakeDocumentNormalizer::default()),
    );
    let result = s
        .pipeline
        .run(&DocumentSource::from_path("/in/a.csv"), 1, None)
        .await;
    assert_eq!(result.unwrap_err(), ParseError::Invalid(DocumentType::Csv));
    assert_eq!(
        *s.log.lock().unwrap(),
        ["parse"],
        "nothing runs after a failure"
    );
}

#[tokio::test]
async fn an_unknown_or_unregistered_format_is_unsupported() {
    let s = setup(
        DocumentType::Csv,
        "/in/a.csv",
        Ok(sample_structured_document(DocumentType::Csv, 1)),
        Box::new(FakeDocumentNormalizer::default()),
    );
    for path in ["/in/a.docx", "/in/a.epub", "/in/sem-extensao"] {
        let result = s
            .pipeline
            .run(&DocumentSource::from_path(path), 1, None)
            .await;
        assert_eq!(result.unwrap_err(), ParseError::Unsupported, "{path}");
    }
    assert!(s.log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_pdf_without_a_text_layer_is_reported_and_not_chunked() {
    let scanned = ParsedDocument::new(
        DocumentType::Pdf,
        DocumentMetadata::default(),
        vec![PageSummary {
            number: 1,
            width: 612.0,
            height: 792.0,
            char_count: 0,
            has_text: false,
        }],
        vec![],
        vec![],
    )
    .unwrap();
    let s = setup(
        DocumentType::Pdf,
        "/in/scan.pdf",
        Ok(scanned),
        Box::new(FakeDocumentNormalizer::default()),
    );
    let processed = s
        .pipeline
        .run(&DocumentSource::from_path("/in/scan.pdf"), 1, None)
        .await
        .unwrap();
    assert!(processed.needs_ocr);
    assert!(processed.chunks.is_empty());
    assert_eq!(processed.pages.len(), 1);
    assert_eq!(*s.log.lock().unwrap(), ["parse", "normalize"]);
}

#[tokio::test]
async fn text_that_normalizes_away_is_empty() {
    struct Blank;
    impl DocumentNormalizer for Blank {
        fn version(&self) -> u32 {
            1
        }
        fn normalize(&self, document: ParsedDocument) -> ParsedDocument {
            let (kind, metadata, pages, mut sections, warnings) = document.into_parts();
            for section in &mut sections {
                for block in &mut section.blocks {
                    block.text = String::new();
                }
            }
            ParsedDocument::new(kind, metadata, pages, sections, warnings).unwrap()
        }
    }
    let s = setup(
        DocumentType::Text,
        "/in/a.txt",
        Ok(sample_structured_document(DocumentType::Text, 2)),
        Box::new(Blank),
    );
    let result = s
        .pipeline
        .run(&DocumentSource::from_path("/in/a.txt"), 1, None)
        .await;
    assert_eq!(result.unwrap_err(), ParseError::Empty);
    assert_eq!(*s.log.lock().unwrap(), ["parse", "normalize"]);
}
