//! Fake `DocumentParser` and the contract every parser must honour.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

use nlmx_application::ports::{BoxFuture, DocumentParser, DocumentSource};
use nlmx_domain::{
    document_type::DocumentType,
    parsed::{
        ContentBlock, ContentKind, DocumentMetadata, DocumentSection, PageSummary, ParseError,
        ParsedDocument,
    },
    source::SourceLocation,
};

/// A valid location of each format, for building sample documents.
pub fn sample_location(kind: DocumentType) -> SourceLocation {
    match kind {
        DocumentType::Pdf => SourceLocation::pdf(1, 1, vec![]),
        DocumentType::Markdown => SourceLocation::markdown(vec![], None),
        DocumentType::Text => SourceLocation::text(0, 5),
        DocumentType::Csv => SourceLocation::csv(1, 1),
        DocumentType::Epub => SourceLocation::epub(1, None, None),
        DocumentType::Docx => SourceLocation::docx(vec![], None),
        DocumentType::Xlsx => SourceLocation::xlsx(1, "Planilha1".into(), 1, 1),
        DocumentType::Note => SourceLocation::note(0, 5),
    }
    .expect("a valid sample location")
}

/// A one-paragraph document of `kind` whose text is `text` (a page is added for a PDF).
pub fn sample_parsed_document(kind: DocumentType, text: &str) -> ParsedDocument {
    let block = ContentBlock {
        kind: ContentKind::Paragraph,
        text: text.into(),
        location: sample_location(kind),
    };
    let pages = if kind.is_paged() {
        vec![PageSummary {
            number: 1,
            width: 612.0,
            height: 792.0,
            char_count: text.chars().count() as u32,
            has_text: true,
        }]
    } else {
        Vec::new()
    };
    ParsedDocument::new(
        kind,
        DocumentMetadata::default(),
        pages,
        vec![DocumentSection::new(None, 0, vec![], vec![block]).expect("one block")],
        Vec::new(),
    )
    .expect("a valid sample document")
}

/// Parser over in-memory results registered by path.
pub struct FakeDocumentParser {
    kind: DocumentType,
    version: u32,
    results: Mutex<HashMap<PathBuf, Result<ParsedDocument, ParseError>>>,
    parsed: Mutex<Vec<PathBuf>>,
}

impl FakeDocumentParser {
    pub fn new(kind: DocumentType) -> Self {
        Self {
            kind,
            version: 97,
            results: Mutex::new(HashMap::new()),
            parsed: Mutex::new(Vec::new()),
        }
    }

    pub fn with_document(self, path: impl Into<PathBuf>, document: ParsedDocument) -> Self {
        self.results
            .lock()
            .unwrap()
            .insert(path.into(), Ok(document));
        self
    }

    pub fn with_error(self, path: impl Into<PathBuf>, error: ParseError) -> Self {
        self.results.lock().unwrap().insert(path.into(), Err(error));
        self
    }

    /// Paths parsed so far, in order.
    pub fn parsed(&self) -> Vec<PathBuf> {
        self.parsed.lock().unwrap().clone()
    }
}

impl DocumentParser for FakeDocumentParser {
    fn document_type(&self) -> DocumentType {
        self.kind
    }

    fn version(&self) -> u32 {
        self.version
    }

    fn parse<'a>(
        &'a self,
        source: &'a DocumentSource,
    ) -> BoxFuture<'a, Result<ParsedDocument, ParseError>> {
        Box::pin(async move {
            if source.document_type.is_some_and(|kind| kind != self.kind) {
                return Err(ParseError::Unsupported);
            }
            self.parsed.lock().unwrap().push(source.path.clone());
            self.results
                .lock()
                .unwrap()
                .get(&source.path)
                .cloned()
                .unwrap_or(Err(ParseError::NotFound))
        })
    }
}

/// Files for [`document_parser_contract`], all with an extension of the parser's format.
pub struct ParserSample<'a> {
    /// A valid document.
    pub valid: &'a Path,
    /// Text that appears in the content of `valid`.
    pub marker: &'a str,
    /// A file that is not a valid document of the format.
    pub invalid: &'a Path,
    /// A path that does not exist.
    pub missing: &'a Path,
}

/// The contract every `DocumentParser` must honour.
pub async fn document_parser_contract(parser: &dyn DocumentParser, sample: ParserSample<'_>) {
    let kind = parser.document_type();

    // Which formats and media types it claims.
    assert!(parser.supports_type(kind));
    for mime in kind.mime_types() {
        assert!(parser.supports_mime(mime), "{mime}");
    }
    assert!(parser.supports_mime(&format!("{}; charset=utf-8", kind.mime_types()[0])));
    for other in DocumentType::ALL.into_iter().filter(|o| *o != kind) {
        assert!(!parser.supports_type(other), "{other}");
        for mime in other.mime_types() {
            assert!(!parser.supports_mime(mime), "{mime}");
        }
    }
    assert!(!parser.supports_mime("application/x-unknown"));

    // A valid document gives a coherent, deterministic result.
    let source = DocumentSource::from_path(sample.valid);
    let parsed = parser
        .parse(&source)
        .await
        .expect("the valid sample parses");
    assert_eq!(parsed.document_type(), kind);
    assert_eq!(parsed.validate(), Ok(()));
    assert!(parsed.has_text(), "a valid sample has text");
    assert!(!parsed.sections().is_empty());
    for section in parsed.sections() {
        assert!(!section.blocks.is_empty(), "sections are never empty");
        assert_eq!(section.location, section.blocks[0].location);
    }
    for block in parsed.blocks() {
        assert_eq!(block.location.document_type(), kind);
    }
    assert!(
        parsed.blocks().any(|b| b.text.contains(sample.marker)),
        "the content of the document is kept"
    );
    assert_eq!(
        !parsed.pages().is_empty(),
        kind.is_paged(),
        "only a paged format reports pages"
    );
    assert_eq!(parser.parse(&source).await.as_ref(), Ok(&parsed));

    // Errors: missing, not a document, wrong declared format.
    let missing = parser
        .parse(&DocumentSource::from_path(sample.missing))
        .await;
    assert_eq!(missing, Err(ParseError::NotFound));

    let invalid = parser
        .parse(&DocumentSource::from_path(sample.invalid))
        .await
        .expect_err("an invalid file does not parse");
    for error in [&missing.unwrap_err(), &invalid] {
        let message = error.to_string();
        assert!(!message.contains(sample.marker), "{message}");
        assert!(
            !message.contains(&*sample.invalid.to_string_lossy()),
            "{message}"
        );
    }

    let other = DocumentType::ALL
        .into_iter()
        .find(|o| *o != kind)
        .expect("more than one format");
    assert_eq!(
        parser
            .parse(&DocumentSource::of_type(sample.valid, other))
            .await,
        Err(ParseError::Unsupported)
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_contract::block_on;

    #[test]
    fn the_fake_parser_honours_the_contract() {
        for kind in DocumentType::ALL {
            let ext = kind.extensions()[0];
            let valid = PathBuf::from(format!("/fake/valid.{ext}"));
            let invalid = PathBuf::from(format!("/fake/invalid.{ext}"));
            let missing = PathBuf::from(format!("/fake/missing.{ext}"));
            let parser = FakeDocumentParser::new(kind)
                .with_document(&valid, sample_parsed_document(kind, "marcador de teste"))
                .with_error(&invalid, ParseError::Invalid(kind));
            block_on(document_parser_contract(
                &parser,
                ParserSample {
                    valid: &valid,
                    marker: "marcador",
                    invalid: &invalid,
                    missing: &missing,
                },
            ));
        }
    }

    #[test]
    fn sample_documents_are_valid_for_every_format() {
        for kind in DocumentType::ALL {
            let parsed = sample_parsed_document(kind, "texto");
            assert_eq!(parsed.validate(), Ok(()));
            assert_eq!(parsed.document_type(), kind);
        }
    }
}
