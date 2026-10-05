//! How a document format looks in the interface: the one place that maps a `DocumentType` to its
//! icon and label. Every list, the chat sources and the source panel use it, so a format is
//! never described twice.

use nlmx_domain::document_type::DocumentType;

/// The icon (an entry of `ds::icon`) and the short label of a format.
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_format_has_its_own_icon_and_label() {
        let views: Vec<FormatView> = DocumentType::ALL.into_iter().map(format_view).collect();
        let mut icons: Vec<_> = views.iter().map(|v| v.icon).collect();
        icons.sort();
        icons.dedup();
        assert_eq!(icons.len(), views.len(), "icons are distinct");
        assert!(views.iter().all(|v| !v.label.is_empty()));
    }
}
