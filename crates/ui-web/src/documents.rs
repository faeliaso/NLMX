//! View models for the document library.

use nlmx_domain::ingestion::{DocumentStatus, DocumentSummary, RemovalImpact};

pub struct DocumentRow {
    pub id: i64,
    /// Not while it is being read (the ingestion would race with the removal).
    pub removable: bool,
    /// What removing it deletes, for the confirmation dialog.
    pub removal: String,
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
        let (status_label, status_kind) = status_badge(doc.status);
        Self {
            id: doc.id,
            removable: !doc.status.is_unfinished(),
            removal: removal_description(&RemovalImpact {
                chunks: doc.chunk_count,
                ..RemovalImpact::default()
            }),
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

/// Label and badge kind of a document status.
pub fn status_badge(status: DocumentStatus) -> (&'static str, &'static str) {
    match status {
        DocumentStatus::Embedding => ("Aguardando embeddings", "info"),
        DocumentStatus::Indexed => ("Indexado", "success"),
        DocumentStatus::NeedsOcr => ("Sem texto (OCR)", "warning"),
        DocumentStatus::Failed => ("Falhou", "danger"),
        _ => ("Processando", "accent"),
    }
}

/// The document list, or why it cannot be shown.
pub enum Library {
    Unavailable(String),
    Documents(Vec<DocumentRow>),
}

/// The confirmation text: what goes, including the chat history, and that the original stays.
pub fn removal_description(impact: &RemovalImpact) -> String {
    let mut text = match impact.chunks {
        0 => "A cópia do documento na biblioteca será apagada deste Mac.".to_string(),
        1 => "A cópia do documento na biblioteca, seu único trecho e os índices de busca serão apagados deste Mac.".to_string(),
        n => format!(
            "A cópia do documento na biblioteca, seus {n} trechos e os índices de busca serão apagados deste Mac."
        ),
    };
    let conversations = match impact.conversations {
        0 => None,
        1 => Some("1 conversa será excluída".to_string()),
        n => Some(format!("{n} conversas serão excluídas")),
    };
    let turns = match impact.turns {
        0 => None,
        1 => Some("1 par de pergunta e resposta que o usou será removido".to_string()),
        n => Some(format!(
            "{n} pares de pergunta e resposta que o usaram serão removidos"
        )),
    };
    let history: Vec<String> = [conversations, turns].into_iter().flatten().collect();
    if !history.is_empty() {
        text.push_str(&format!(" No Chat, {}.", history.join(" e ")));
    }
    text.push_str(" O arquivo original não é afetado.");
    text
}

/// A message shown above the document list after an action.
pub struct Notice {
    pub kind: &'static str,
    pub title: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_what_a_removal_takes() {
        let only_file = removal_description(&RemovalImpact::default());
        assert_eq!(
            only_file,
            "A cópia do documento na biblioteca será apagada deste Mac. O arquivo original não é afetado."
        );
        let one = removal_description(&RemovalImpact {
            chunks: 1,
            conversations: 1,
            turns: 1,
        });
        assert_eq!(
            one,
            "A cópia do documento na biblioteca, seu único trecho e os índices de busca serão apagados deste Mac. \
             No Chat, 1 conversa será excluída e 1 par de pergunta e resposta que o usou será removido. \
             O arquivo original não é afetado."
        );
        let many = removal_description(&RemovalImpact {
            chunks: 42,
            conversations: 0,
            turns: 5,
        });
        assert!(many.contains("seus 42 trechos"), "{many}");
        assert!(
            many.contains("No Chat, 5 pares de pergunta e resposta que o usaram serão removidos."),
            "{many}"
        );
    }
}
