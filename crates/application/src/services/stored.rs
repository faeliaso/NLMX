//! From the pipeline's result to what is persisted. Kept apart from the pipeline itself: the
//! status a stored document gets (waiting for embeddings, or needing OCR) is a persistence
//! concern, and the pipeline stops before embeddings.

use nlmx_domain::ingestion::DocumentStatus;

use super::pipeline::ProcessedDocument;
use crate::ports::{PageRecord, StoredExtraction};

impl ProcessedDocument {
    /// What `DocumentRepository::save_processed` stores. `title` is the document's display name
    /// (the pipeline does not choose one: it is the metadata title or, failing that, the caller's
    /// fallback such as the file name). The document waits for embeddings, or needs OCR.
    pub fn into_stored(self, title: String) -> StoredExtraction {
        StoredExtraction {
            title,
            metadata: self.metadata,
            pages: self
                .pages
                .into_iter()
                .map(|p| PageRecord {
                    number: p.number,
                    width: p.width,
                    height: p.height,
                    char_count: p.char_count,
                    has_text: p.has_text,
                })
                .collect(),
            outline: self.outline,
            chunks: self.chunks,
            extractor_version: self.versions.parser,
            normalizer_version: self.versions.normalizer,
            chunker_version: self.versions.chunker,
            status: if self.needs_ocr {
                DocumentStatus::NeedsOcr
            } else {
                DocumentStatus::Embedding
            },
        }
    }
}
