//! The state of the search index: where each document is in the pipeline and how many chunks
//! have vectors in the active embedding model.

use crate::ingestion::{DocumentId, DocumentStatus};

/// A document's ingestion job, as shown in the Indexação section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexJob {
    pub document_id: DocumentId,
    pub title: String,
    pub document_type: crate::document_type::DocumentType,
    pub status: DocumentStatus,
    pub error: Option<String>,
    pub chunks: u32,
    /// How many times the pipeline started reading the document.
    pub attempts: u32,
    /// ISO-8601 timestamps of the last run (`None` before the first one).
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    /// `finished_at − started_at`, when both are known.
    pub duration_ms: Option<u64>,
}

/// Chunks with a vector in one embedding model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedModel {
    pub model_id: String,
    pub dimensions: u32,
    pub chunks: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexSnapshot {
    /// One job per document, most recently imported first.
    pub jobs: Vec<IndexJob>,
    /// Chunks of every document.
    pub chunks: u32,
    /// `None` when the asked model has no vectors yet (or no model was asked).
    pub model: Option<EmbeddedModel>,
}

impl IndexSnapshot {
    pub fn count(&self, status: DocumentStatus) -> usize {
        self.jobs.iter().filter(|j| j.status == status).count()
    }

    /// Documents still being read (extraction, structure, chunks).
    pub fn reading(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| j.status.is_unfinished())
            .count()
    }

    /// Chunks of documents waiting for embeddings.
    pub fn pending_chunks(&self) -> u32 {
        self.jobs
            .iter()
            .filter(|j| j.status == DocumentStatus::Embedding)
            .map(|j| j.chunks)
            .sum()
    }

    pub fn embedded_chunks(&self) -> u32 {
        self.model.as_ref().map_or(0, |m| m.chunks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: DocumentId, status: DocumentStatus, chunks: u32) -> IndexJob {
        IndexJob {
            document_id: id,
            title: format!("doc {id}"),
            document_type: crate::document_type::DocumentType::Pdf,
            status,
            error: None,
            chunks,
            attempts: 1,
            started_at: None,
            finished_at: None,
            duration_ms: None,
        }
    }

    #[test]
    fn counts_documents_and_chunks_by_state() {
        let snapshot = IndexSnapshot {
            jobs: vec![
                job(1, DocumentStatus::Indexed, 10),
                job(2, DocumentStatus::Embedding, 4),
                job(3, DocumentStatus::Embedding, 3),
                job(4, DocumentStatus::Extracting, 0),
                job(5, DocumentStatus::Failed, 0),
            ],
            chunks: 17,
            model: Some(EmbeddedModel {
                model_id: "m".into(),
                dimensions: 8,
                chunks: 10,
            }),
        };
        assert_eq!(snapshot.count(DocumentStatus::Embedding), 2);
        assert_eq!(snapshot.count(DocumentStatus::Failed), 1);
        assert_eq!(snapshot.reading(), 1);
        assert_eq!(snapshot.pending_chunks(), 7);
        assert_eq!(snapshot.embedded_chunks(), 10);
        assert_eq!(IndexSnapshot::default().embedded_chunks(), 0);
    }
}
