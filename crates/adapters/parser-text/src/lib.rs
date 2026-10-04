//! `DocumentParser` adapters for plain-text formats: Markdown, TXT and CSV.
//!
//! Each parser has a pure `parse_bytes` (no I/O, easy to test); the trait implementation reads
//! the file on a blocking thread and calls it. Parsers never log or return the document's
//! content, name or path.

mod csv_parser;
mod decode;
mod lines;
mod markdown;
mod sniff;
mod stream;
mod text;

use std::{io, path::Path};

use nlmx_application::ports::{BoxFuture, DocumentSource};
use nlmx_domain::{
    document_type::DocumentType,
    parsed::{
        ContentBlock, DocumentMetadata, DocumentSection, ParseError, ParseWarning, ParsedDocument,
    },
};

pub use csv_parser::{CsvDocumentParser, MAX_ROWS};
pub use decode::MAX_BYTES;
pub use markdown::MarkdownDocumentParser;
pub use stream::{CsvLimits, CsvStream, MAX_STREAM_BYTES, MAX_STREAM_ROWS};
pub use text::TextDocumentParser;

/// Reads `path` (at most [`MAX_BYTES`]) and parses it on a blocking thread.
fn parse_file<'a>(
    kind: DocumentType,
    source: &'a DocumentSource,
    parse: fn(&[u8]) -> Result<ParsedDocument, ParseError>,
) -> BoxFuture<'a, Result<ParsedDocument, ParseError>> {
    Box::pin(async move {
        if source
            .document_type
            .is_some_and(|declared| declared != kind)
        {
            return Err(ParseError::Unsupported);
        }
        let path = source.path.clone();
        tokio::task::spawn_blocking(move || parse(&read(&path)?))
            .await
            .map_err(|_| ParseError::Engine("a leitura foi interrompida".into()))?
    })
}

fn read(path: &Path) -> Result<Vec<u8>, ParseError> {
    let io_error = |error: io::Error| match error.kind() {
        io::ErrorKind::NotFound => ParseError::NotFound,
        _ => ParseError::Engine("não foi possível ler o arquivo".into()),
    };
    let metadata = std::fs::metadata(path).map_err(io_error)?;
    if !metadata.is_file() {
        return Err(ParseError::NotFound);
    }
    if metadata.len() > MAX_BYTES {
        return Err(ParseError::TooLarge);
    }
    std::fs::read(path).map_err(io_error)
}

/// One untitled section with all the blocks (formats without headings).
fn flat_document(
    kind: DocumentType,
    metadata: DocumentMetadata,
    blocks: Vec<ContentBlock>,
    warnings: Vec<ParseWarning>,
) -> Result<ParsedDocument, ParseError> {
    let sections = DocumentSection::new(None, 0, Vec::new(), blocks)
        .into_iter()
        .collect();
    ParsedDocument::new(kind, metadata, Vec::new(), sections, warnings)
        .map_err(|_| ParseError::Invalid(kind))
}
