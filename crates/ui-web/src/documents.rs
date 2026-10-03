//! View models for the document library.

use nlmx_domain::ingestion::{DocumentStatus, DocumentSummary};

pub struct DocumentRow {
    pub id: i64,
    /// Has extracted pages, so the viewer can open it.
    pub viewable: bool,
    pub title: String,
    pub filename: String,
    pub pages: String,
    pub chunks: String,
    pub status_label: &'static str,
    pub status_kind: &'static str,
    pub error: String,
    pub imported_on: String,
}

impl From<DocumentSummary> for DocumentRow {
    fn from(doc: DocumentSummary) -> Self {
        let (status_label, status_kind) = match doc.status {
            DocumentStatus::Embedding => ("Aguardando embeddings", "info"),
            DocumentStatus::Indexed => ("Indexado", "success"),
            DocumentStatus::NeedsOcr => ("Sem texto (OCR)", "warning"),
            DocumentStatus::Failed => ("Falhou", "danger"),
            _ => ("Processando", "accent"),
        };
        Self {
            id: doc.id,
            viewable: doc.page_count.is_some_and(|n| n > 0)
                && !matches!(doc.status, DocumentStatus::Failed),
            pages: match doc.page_count {
                Some(1) => "1 página".into(),
                Some(n) => format!("{n} páginas"),
                None => "—".into(),
            },
            title: doc.title,
            filename: doc.original_filename,
            chunks: match doc.chunk_count {
                1 => "1 trecho".into(),
                n => format!("{n} trechos"),
            },
            status_label,
            status_kind,
            error: doc.error.unwrap_or_default(),
            // "2026-10-02T14:31:05.123Z" → "02/10/2026"
            imported_on: match (
                doc.imported_at.get(0..4),
                doc.imported_at.get(5..7),
                doc.imported_at.get(8..10),
            ) {
                (Some(y), Some(m), Some(d)) => format!("{d}/{m}/{y}"),
                _ => doc.imported_at,
            },
        }
    }
}

/// The document list, or why it cannot be shown.
pub enum Library {
    Unavailable(String),
    Documents(Vec<DocumentRow>),
}
