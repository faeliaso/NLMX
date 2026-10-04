//! The EPUB `DocumentParser`: metadata from the package document, one chapter at a time as
//! typed blocks located by chapter (and section), with the chapter titles of the table of
//! contents. Protected (DRM) books are refused; encryption that only obfuscates fonts is not.
//!
//! The parser works on bytes in memory (`parse_bytes`); `EpubDocumentParser` reads the file
//! and runs it on a blocking thread. Nothing here logs or reports the book's content, name
//! or path.

mod archive;
mod book;
mod opf;
mod toc;
mod xhtml;
mod xml;

use std::{io::ErrorKind, path::Path};

use nlmx_application::ports::{BoxFuture, DocumentParser, DocumentSource};
use nlmx_domain::{
    document_type::DocumentType,
    parsed::{ParseError, ParsedDocument},
};

pub use archive::Limits;
pub use book::{parse_bytes, parse_bytes_with_limits};

/// Version of the parser's output; bump it when the same book would parse differently.
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Default)]
pub struct EpubDocumentParser {
    limits: Limits,
}

impl EpubDocumentParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_limits(limits: Limits) -> Self {
        Self { limits }
    }
}

fn parse_file(path: &Path, limits: Limits) -> Result<ParsedDocument, ParseError> {
    let size = std::fs::metadata(path).map_err(io_error)?.len();
    if size > limits.max_total_bytes {
        return Err(ParseError::TooLarge);
    }
    let bytes = std::fs::read(path).map_err(io_error)?;
    parse_bytes_with_limits(&bytes, limits)
}

fn io_error(error: std::io::Error) -> ParseError {
    match error.kind() {
        ErrorKind::NotFound => ParseError::NotFound,
        _ => ParseError::Engine("falha ao ler o arquivo".into()),
    }
}

impl DocumentParser for EpubDocumentParser {
    fn document_type(&self) -> DocumentType {
        DocumentType::Epub
    }

    fn version(&self) -> u32 {
        VERSION
    }

    fn parse<'a>(
        &'a self,
        source: &'a DocumentSource,
    ) -> BoxFuture<'a, Result<ParsedDocument, ParseError>> {
        Box::pin(async move {
            if source
                .document_type
                .is_some_and(|kind| kind != DocumentType::Epub)
            {
                return Err(ParseError::Unsupported);
            }
            let path = source.path.clone();
            let limits = self.limits;
            tokio::task::spawn_blocking(move || parse_file(&path, limits))
                .await
                .map_err(|_| ParseError::Engine("falha interna ao ler o documento".into()))?
        })
    }
}
