//! Embeds the chunks of ingested documents and indexes them in the vector store.

use std::{sync::Arc, time::Instant};

use nlmx_domain::{
    embedding::EmbeddingPurpose,
    ingestion::{DocumentId, DocumentStatus},
    telemetry::{ErrorKind, Measurement},
    vectors::{DeleteScope, EmbeddingSpace},
};

use crate::{
    ports::{ChunkReader, DocumentRepository, EmbeddingSource, VectorStore},
    telemetry::{ms, record},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedOutcome {
    Indexed {
        chunks: u32,
    },
    /// No embedding model is configured; the document keeps waiting.
    NoModel,
    /// The model failed; the document stays pending (status `embedding`, with the reason).
    Failed(String),
}

pub struct EmbedDocuments {
    pub embeddings: Arc<dyn EmbeddingSource>,
    pub vectors: Arc<dyn VectorStore>,
    pub chunks: Arc<dyn ChunkReader>,
    pub documents: Arc<dyn DocumentRepository>,
    /// Chunks per embedding request batch.
    pub batch_size: usize,
}

impl EmbedDocuments {
    /// Embeds all chunks of a document with the current model and marks it `indexed`.
    pub async fn embed_document(&self, id: DocumentId) -> EmbedOutcome {
        let Some(provider) = self.embeddings.current() else {
            return EmbedOutcome::NoModel;
        };
        let started = Instant::now();
        match self.run(id, provider.as_ref()).await {
            Ok((chunks, dims)) => {
                record(&Measurement::Embedded {
                    document_id: id,
                    chunks,
                    dims,
                    total_ms: ms(started),
                });
                EmbedOutcome::Indexed { chunks }
            }
            Err((reason, kind)) => {
                record(&Measurement::EmbedFailed {
                    document_id: id,
                    kind,
                    total_ms: ms(started),
                });
                let message = format!("Embeddings pendentes: {reason}");
                let _ = self
                    .documents
                    .set_status(id, DocumentStatus::Embedding, Some(message))
                    .await;
                EmbedOutcome::Failed(reason)
            }
        }
    }

    async fn run(
        &self,
        id: DocumentId,
        provider: &dyn crate::ports::EmbeddingProvider,
    ) -> Result<(u32, u32), (String, ErrorKind)> {
        let model =
            |e: nlmx_domain::embedding::EmbeddingError| (e.to_string(), ErrorKind::Unavailable);
        let store = |e: String| (e, ErrorKind::Storage);
        let identity = provider.identity().await.map_err(model)?;
        // The dimension comes from the loaded model.
        let index = self
            .vectors
            .create_index(&EmbeddingSpace::from_identity(&identity))
            .await
            .map_err(|e| store(e.to_string()))?;
        let chunks = self
            .chunks
            .document_chunks(id)
            .await
            .map_err(|e| store(e.message))?;
        // Re-embedding replaces whatever this document had in the index.
        self.vectors
            .delete(index.id, &DeleteScope::Document(id))
            .await
            .map_err(|e| store(e.to_string()))?;
        for batch in chunks.chunks(self.batch_size.max(1)) {
            let inputs: Vec<String> = batch
                .iter()
                .map(|c| match &c.section {
                    Some(section) => format!("{section}\n{}", c.text),
                    None => c.text.clone(),
                })
                .collect();
            let vectors = provider
                .embed_batch(&inputs, EmbeddingPurpose::Passage)
                .await
                .map_err(model)?;
            let items: Vec<(i64, Vec<f32>)> =
                batch.iter().map(|c| c.chunk_id).zip(vectors).collect();
            self.vectors
                .insert_batch(index.id, &items)
                .await
                .map_err(|e| store(e.to_string()))?;
        }
        self.documents
            .set_status(id, DocumentStatus::Indexed, None)
            .await
            .map_err(|e| store(e.message))?;
        Ok((chunks.len() as u32, identity.dimensions))
    }

    /// Embeds every document waiting for embeddings (after startup or a model change).
    pub async fn embed_pending(&self) -> Vec<(DocumentId, EmbedOutcome)> {
        if self.embeddings.current().is_none() {
            return Vec::new();
        }
        let Ok(documents) = self.documents.list().await else {
            return Vec::new();
        };
        let mut outcomes = Vec::new();
        for doc in documents
            .into_iter()
            .filter(|d| d.status == DocumentStatus::Embedding)
        {
            outcomes.push((doc.id, self.embed_document(doc.id).await));
        }
        outcomes
    }

    /// Marks indexed documents as pending again (e.g. the active model changed) and re-embeds them.
    pub async fn reindex_all(&self) -> Vec<(DocumentId, EmbedOutcome)> {
        if let Ok(documents) = self.documents.list().await {
            for doc in documents
                .into_iter()
                .filter(|d| d.status == DocumentStatus::Indexed)
            {
                let _ = self
                    .documents
                    .set_status(doc.id, DocumentStatus::Embedding, None)
                    .await;
            }
        }
        self.embed_pending().await
    }
}
