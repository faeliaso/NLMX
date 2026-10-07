//! How a document format looks in the interface: the one place that maps a `DocumentType` to its
//! icon and label. Every list, the chat sources and the source panel use it, so a format is
//! never described twice.

use nlmx_domain::document_type::DocumentType;
use nlmx_i18n::{Locale, t};

/// The icon (an entry of `ds::icon`) and the short label ("DOCX"). What a format is called in
/// full ("Microsoft Word document") for the source panel is [`format_description`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatView {
    pub icon: &'static str,
    pub label: &'static str,
}

pub fn format_view(kind: DocumentType) -> FormatView {
    match kind {
        DocumentType::Pdf => FormatView {
            icon: "format-pdf",
            label: "PDF",
        },
        DocumentType::Markdown => FormatView {
            icon: "format-markdown",
            label: "Markdown",
        },
        DocumentType::Text => FormatView {
            icon: "format-text",
            label: "TXT",
        },
        DocumentType::Csv => FormatView {
            icon: "format-csv",
            label: "CSV",
        },
        DocumentType::Epub => FormatView {
            icon: "format-epub",
            label: "EPUB",
        },
        DocumentType::Docx => FormatView {
            icon: "format-docx",
            label: "DOCX",
        },
        DocumentType::Xlsx => FormatView {
            icon: "format-xlsx",
            label: "XLSX",
        },
        DocumentType::Note => FormatView {
            icon: "format-note",
            // The only label that is a word and not a product or file type.
            label: match nlmx_i18n::current() {
                Locale::En => "Note",
                Locale::PtBr | Locale::Es => "Nota",
            },
        },
    }
}

/// What the format is called in full, in the interface language ("Documento do Microsoft Word").
pub fn format_description(kind: DocumentType) -> String {
    t(match kind {
        DocumentType::Pdf => "sources-format-pdf",
        DocumentType::Markdown => "sources-format-markdown",
        DocumentType::Text => "sources-format-text",
        DocumentType::Csv => "sources-format-csv",
        DocumentType::Epub => "sources-format-epub",
        DocumentType::Docx => "sources-format-docx",
        DocumentType::Xlsx => "sources-format-xlsx",
        DocumentType::Note => "sources-format-note",
    })
}
