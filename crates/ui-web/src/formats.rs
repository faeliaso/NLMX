//! How a document format looks in the interface: the one place that maps a `DocumentType` to its
//! icon and label. Every list, the chat sources and the source panel use it, so a format is
//! never described twice.

use nlmx_domain::document_type::DocumentType;

/// The icon (an entry of `ds::icon`), the short label ("DOCX") and what the format is called in
/// full ("Documento do Microsoft Word") for the source panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatView {
    pub icon: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

pub fn format_view(kind: DocumentType) -> FormatView {
    match kind {
        DocumentType::Pdf => FormatView {
            icon: "format-pdf",
            label: "PDF",
            description: "Documento PDF",
        },
        DocumentType::Markdown => FormatView {
            icon: "format-markdown",
            label: "Markdown",
            description: "Documento Markdown",
        },
        DocumentType::Text => FormatView {
            icon: "format-text",
            label: "TXT",
            description: "Arquivo de texto",
        },
        DocumentType::Csv => FormatView {
            icon: "format-csv",
            label: "CSV",
            description: "Tabela CSV/TSV (valores separados)",
        },
        DocumentType::Epub => FormatView {
            icon: "format-epub",
            label: "EPUB",
            description: "Livro digital EPUB",
        },
        DocumentType::Docx => FormatView {
            icon: "format-docx",
            label: "DOCX",
            description: "Documento do Microsoft Word",
        },
        DocumentType::Xlsx => FormatView {
            icon: "format-xlsx",
            label: "XLSX",
            description: "Planilha do Microsoft Excel",
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
        assert!(
            views
                .iter()
                .all(|v| !v.label.is_empty() && !v.description.is_empty())
        );
    }
}
