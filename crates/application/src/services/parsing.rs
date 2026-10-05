//! Parsing: the registry that picks the parser of a format, and the PDF parser (the one parser
//! built from other ports; the rest live in adapters).

mod pdf;

use std::sync::Arc;

use nlmx_domain::{
    document_type::DocumentType,
    parsed::{ParseError, ParsedDocument},
};

pub use pdf::PdfDocumentParser;
pub(crate) use pdf::read_layouts;

use crate::ports::{DocumentParser, DocumentSource};

/// The parsers of the app, by format. Adding a format is registering a parser here (in the
/// composition root); nothing downstream of [`ParsedDocument`] changes.
#[derive(Default, Clone)]
pub struct ParserRegistry {
    parsers: Vec<Arc<dyn DocumentParser>>,
}

impl ParserRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a parser, replacing the one of the same format if there is one.
    pub fn register(&mut self, parser: Arc<dyn DocumentParser>) -> &mut Self {
        let kind = parser.document_type();
        self.parsers.retain(|p| p.document_type() != kind);
        self.parsers.push(parser);
        self
    }

    pub fn with(mut self, parser: Arc<dyn DocumentParser>) -> Self {
        self.register(parser);
        self
    }

    pub fn parser_for(&self, document_type: DocumentType) -> Option<&Arc<dyn DocumentParser>> {
        self.parsers.iter().find(|p| p.supports_type(document_type))
    }

    pub fn parser_for_mime(&self, mime: &str) -> Option<&Arc<dyn DocumentParser>> {
        self.parsers.iter().find(|p| p.supports_mime(mime))
    }

    /// The formats that have a parser, in `DocumentType::ALL` order.
    pub fn supported_types(&self) -> Vec<DocumentType> {
        DocumentType::ALL
            .into_iter()
            .filter(|kind| self.parser_for(*kind).is_some())
            .collect()
    }

    /// The parser for a file and the source with its format resolved (the declared one, or the
    /// one of its extension). `Unsupported` when the format is unknown or has no parser.
    pub fn resolve(
        &self,
        source: &DocumentSource,
    ) -> Result<(&Arc<dyn DocumentParser>, DocumentSource), ParseError> {
        let kind = source
            .document_type
            .or_else(|| DocumentType::from_path(&source.path))
            .ok_or(ParseError::Unsupported)?;
        let parser = self.parser_for(kind).ok_or(ParseError::Unsupported)?;
        let mut resolved = DocumentSource::of_type(source.path.clone(), kind);
        resolved.text = source.text.clone();
        Ok((parser, resolved))
    }

    /// Parses a file with the parser of its format. `Unsupported` when the format is unknown
    /// or has no parser.
    pub async fn parse(&self, source: &DocumentSource) -> Result<ParsedDocument, ParseError> {
        let (parser, resolved) = self.resolve(source)?;
        parser.parse(&resolved).await
    }
}
