//! The Office `DocumentParser`s: DOCX (Word) and XLSX (Excel), both OOXML packages (a zip of
//! XML parts) read with the same limits against zip bombs.
//!
//! - DOCX: headings, paragraphs, list items and tables, located by the enclosing headings and
//!   the paragraph number (`SourceLocation::Docx`).
//! - XLSX: one section per visible sheet whose rows are `Record` blocks, located by sheet and
//!   Excel row number (`SourceLocation::Xlsx`). The chunker keeps each sheet's columns.
//!
//! Neither has a viewer: both are indexed and cited, and sources open the source panel.
//! Password-protected files (and the old `.doc`/`.xls`, which share their container) are
//! reported as `ParseError::Encrypted`. The parsers work on bytes in memory
//! (`parse_docx`, `parse_xlsx`); the adapters read the file on a blocking thread. Nothing here
//! logs or reports the content, name or path of the file.

mod archive;
mod docx;
mod numfmt;
mod xlsx;
mod xml;

use std::{io::ErrorKind, path::Path};

use nlmx_application::ports::{BoxFuture, DocumentParser, DocumentSource};
use nlmx_domain::{
    document_type::DocumentType,
    parsed::{ParseError, ParsedDocument},
};

pub use archive::Limits;
pub use docx::{parse_docx, parse_docx_with_limits};
pub use xlsx::{MAX_ROWS, parse_xlsx, parse_xlsx_with_limits};

/// Version of the DOCX parser's output; bump it when the same file would parse differently.
pub const DOCX_VERSION: u32 = 3;
/// Version of the XLSX parser's output.
pub const XLSX_VERSION: u32 = 2;

fn read_file(path: &Path, limits: Limits) -> Result<Vec<u8>, ParseError> {
    let size = std::fs::metadata(path).map_err(io_error)?.len();
    if size > limits.max_total_bytes {
        return Err(ParseError::TooLarge);
    }
    std::fs::read(path).map_err(io_error)
}

fn io_error(error: std::io::Error) -> ParseError {
    match error.kind() {
        ErrorKind::NotFound => ParseError::NotFound,
        _ => ParseError::Engine("falha ao ler o arquivo".into()),
    }
}

macro_rules! office_parser {
    ($name:ident, $kind:expr, $version:expr, $parse:ident) => {
        #[derive(Debug, Clone, Copy, Default)]
        pub struct $name {
            limits: Limits,
        }

        impl $name {
            pub fn new() -> Self {
                Self::default()
            }

            pub fn with_limits(limits: Limits) -> Self {
                Self { limits }
            }
        }

        impl DocumentParser for $name {
            fn document_type(&self) -> DocumentType {
                $kind
            }

            fn version(&self) -> u32 {
                $version
            }

            fn parse<'a>(
                &'a self,
                source: &'a DocumentSource,
            ) -> BoxFuture<'a, Result<ParsedDocument, ParseError>> {
                Box::pin(async move {
                    if source.document_type.is_some_and(|kind| kind != $kind) {
                        return Err(ParseError::Unsupported);
                    }
                    let path = source.path.clone();
                    let limits = self.limits;
                    tokio::task::spawn_blocking(move || {
                        let bytes = read_file(&path, limits)?;
                        $parse(&bytes, limits)
                    })
                    .await
                    .map_err(|_| ParseError::Engine("falha interna ao ler o documento".into()))?
                })
            }
        }
    };
}

office_parser!(
    DocxDocumentParser,
    DocumentType::Docx,
    DOCX_VERSION,
    parse_docx_with_limits
);
office_parser!(
    XlsxDocumentParser,
    DocumentType::Xlsx,
    XLSX_VERSION,
    parse_xlsx_with_limits
);
