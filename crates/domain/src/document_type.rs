//! The formats NLMX can import. Every one of them can be indexed and used by the RAG;
//! only some can also be previewed in the interface.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// A supported document format. Never compare format names as loose strings: use this type
/// (`as_str` is the stable name stored and exchanged).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentType {
    Pdf,
    Markdown,
    Text,
    Csv,
    Epub,
    Docx,
    Xlsx,
}

/// A name that is not one of `DocumentType::as_str`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownDocumentType(pub String);

impl fmt::Display for UnknownDocumentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tipo de documento desconhecido: {}", self.0)
    }
}

impl std::error::Error for UnknownDocumentType {}

impl DocumentType {
    pub const ALL: [DocumentType; 7] = [
        Self::Pdf,
        Self::Markdown,
        Self::Text,
        Self::Csv,
        Self::Epub,
        Self::Docx,
        Self::Xlsx,
    ];

    /// Stable name (same as the serde representation).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Markdown => "markdown",
            Self::Text => "text",
            Self::Csv => "csv",
            Self::Epub => "epub",
            Self::Docx => "docx",
            Self::Xlsx => "xlsx",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }

    /// File extensions of the format, lowercase and without the dot.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Pdf => &["pdf"],
            Self::Markdown => &["md", "markdown"],
            Self::Text => &["txt", "text"],
            Self::Csv => &["csv", "tsv"],
            Self::Epub => &["epub"],
            Self::Docx => &["docx"],
            Self::Xlsx => &["xlsx"],
        }
    }

    /// The format of a file extension (case-insensitive, with or without the leading dot).
    pub fn from_extension(extension: &str) -> Option<Self> {
        let extension = extension.trim_start_matches('.').to_lowercase();
        Self::ALL
            .into_iter()
            .find(|kind| kind.extensions().contains(&extension.as_str()))
    }

    /// The format of a file, by its extension.
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        path.extension()
            .and_then(|extension| extension.to_str())
            .and_then(Self::from_extension)
    }

    /// Media types of the format, the canonical one first.
    pub fn mime_types(self) -> &'static [&'static str] {
        match self {
            Self::Pdf => &["application/pdf"],
            Self::Markdown => &["text/markdown", "text/x-markdown"],
            Self::Text => &["text/plain"],
            Self::Csv => &[
                "text/csv",
                "application/csv",
                "text/tab-separated-values",
                "text/tsv",
            ],
            Self::Epub => &["application/epub+zip"],
            Self::Docx => {
                &["application/vnd.openxmlformats-officedocument.wordprocessingml.document"]
            }
            Self::Xlsx => &["application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"],
        }
    }

    /// The format of a media type (case-insensitive; parameters such as `; charset=utf-8`
    /// are ignored).
    pub fn from_mime(mime: &str) -> Option<Self> {
        let essence = mime
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        Self::ALL
            .into_iter()
            .find(|kind| kind.mime_types().contains(&essence.as_str()))
    }

    /// Name for messages to the user ("PDF", "Markdown", "TXT", ...).
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Markdown => "Markdown",
            Self::Text => "TXT",
            Self::Csv => "CSV",
            Self::Epub => "EPUB",
            Self::Docx => "DOCX",
            Self::Xlsx => "XLSX",
        }
    }

    /// Whether the interface can show the document itself (the PDF viewer).
    ///
    /// This says nothing about indexing: every `DocumentType` is indexed and answers
    /// questions; a format that is not previewable only has no viewer.
    pub fn previewable(self) -> bool {
        matches!(self, Self::Pdf)
    }

    /// Whether the content is divided into numbered pages (so locations have a page).
    pub fn is_paged(self) -> bool {
        matches!(self, Self::Pdf)
    }
}

impl fmt::Display for DocumentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DocumentType {
    type Err = UnknownDocumentType;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or_else(|| UnknownDocumentType(value.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for kind in DocumentType::ALL {
            assert_eq!(DocumentType::parse(kind.as_str()), Some(kind));
            assert_eq!(kind.as_str().parse::<DocumentType>(), Ok(kind));
            assert_eq!(kind.to_string(), kind.as_str());
        }
        assert_eq!(DocumentType::parse("PDF"), None);
        assert_eq!(
            "odt".parse::<DocumentType>(),
            Err(UnknownDocumentType("odt".into()))
        );
    }

    #[test]
    fn all_lists_each_type_once() {
        let names: Vec<_> = DocumentType::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(
            names,
            ["pdf", "markdown", "text", "csv", "epub", "docx", "xlsx"]
        );
    }

    #[test]
    fn extensions_resolve_to_their_type() {
        let cases = [
            ("pdf", DocumentType::Pdf),
            ("PDF", DocumentType::Pdf),
            ("md", DocumentType::Markdown),
            ("Markdown", DocumentType::Markdown),
            (".txt", DocumentType::Text),
            ("text", DocumentType::Text),
            ("csv", DocumentType::Csv),
            ("tsv", DocumentType::Csv),
            (".TSV", DocumentType::Csv),
            (".EPUB", DocumentType::Epub),
            ("docx", DocumentType::Docx),
            (".XLSX", DocumentType::Xlsx),
        ];
        for (extension, expected) in cases {
            assert_eq!(DocumentType::from_extension(extension), Some(expected));
        }
        for unknown in ["", "odt", "html", "pdfx", "p.df"] {
            assert_eq!(DocumentType::from_extension(unknown), None);
        }
    }

    #[test]
    fn paths_resolve_by_extension() {
        use std::path::Path;
        assert_eq!(
            DocumentType::from_path(Path::new("/tmp/Relatório.PDF")),
            Some(DocumentType::Pdf)
        );
        assert_eq!(
            DocumentType::from_path(Path::new("notas.md")),
            Some(DocumentType::Markdown)
        );
        assert_eq!(DocumentType::from_path(Path::new("sem-extensao")), None);
        assert_eq!(DocumentType::from_path(Path::new("a.odt")), None);
    }

    #[test]
    fn media_types_resolve_to_their_type() {
        for kind in DocumentType::ALL {
            for mime in kind.mime_types() {
                assert_eq!(DocumentType::from_mime(mime), Some(kind), "{mime}");
            }
        }
        assert_eq!(
            DocumentType::from_mime("Text/CSV; charset=utf-8"),
            Some(DocumentType::Csv)
        );
        assert_eq!(
            DocumentType::from_mime(" application/pdf "),
            Some(DocumentType::Pdf)
        );
        assert_eq!(DocumentType::from_mime("application/zip"), None);
        assert_eq!(DocumentType::from_mime(""), None);
    }

    #[test]
    fn display_names_are_set_for_every_type() {
        let names: Vec<_> = DocumentType::ALL.iter().map(|k| k.display_name()).collect();
        assert_eq!(
            names,
            ["PDF", "Markdown", "TXT", "CSV", "EPUB", "DOCX", "XLSX"]
        );
    }

    #[test]
    fn extensions_are_lowercase_and_not_shared() {
        let mut seen = Vec::new();
        for kind in DocumentType::ALL {
            assert!(!kind.extensions().is_empty());
            for extension in kind.extensions() {
                assert_eq!(*extension, extension.to_lowercase());
                assert!(!extension.starts_with('.'));
                assert!(!seen.contains(extension), "{extension} repeats");
                seen.push(*extension);
            }
        }
    }

    #[test]
    fn only_pdf_can_be_previewed() {
        for kind in DocumentType::ALL {
            assert_eq!(kind.previewable(), kind == DocumentType::Pdf, "{kind}");
            assert_eq!(kind.is_paged(), kind == DocumentType::Pdf, "{kind}");
        }
    }

    #[test]
    fn every_type_is_importable_regardless_of_preview() {
        // Preview never gates indexing: each format is reachable from a file extension.
        for kind in DocumentType::ALL {
            let extension = kind.extensions()[0];
            assert_eq!(DocumentType::from_extension(extension), Some(kind));
        }
    }

    #[test]
    fn serde_uses_the_stable_names() {
        for kind in DocumentType::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            assert_eq!(serde_json::from_str::<DocumentType>(&json).unwrap(), kind);
        }
        assert!(serde_json::from_str::<DocumentType>("\"odt\"").is_err());
        assert!(serde_json::from_str::<DocumentType>("\"Pdf\"").is_err());
    }
}
