//! Hybrid retrieval: query → normalization → vector search → FTS search → union →
//! score normalization → weighted ranking (see `nlmx_domain::retrieval::fuse`).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

use nlmx_domain::{
    embedding::EmbeddingPurpose,
    retrieval::{
        ComponentScore, LexicalCandidate, LexicalQuery, RetrievalOptions, SemanticCandidate,
        coverage, fuse, normalize_query,
    },
    telemetry::{ErrorKind, Measurement},
    vectors::{EmbeddingSpace, VectorFilter, VectorIndexId},
};

use crate::telemetry::{ms, record};

use crate::ports::{
    ChunkReader, ChunkView, EmbeddingProvider, EmbeddingSource, LexicalIndex, VectorStore,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetrievalMode {
    Hybrid,
    SemanticOnly,
    LexicalOnly,
    /// Passages chosen by document structure (overview, a numbered section), not by search.
    Structure,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetrievedChunk {
    pub chunk: ChunkView,
    /// Final score in [0, 1].
    pub score: f32,
    pub semantic: Option<ComponentScore>,
    pub lexical: Option<ComponentScore>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalResult {
    pub chunks: Vec<RetrievedChunk>,
    pub mode: RetrievalMode,
    /// Why a mechanism was skipped (e.g. embedding model unavailable).
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetrievalError {
    InvalidOptions(String),
    /// Neither mechanism could run.
    Unavailable(String),
}

impl std::fmt::Display for RetrievalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidOptions(m) => write!(f, "opções de busca inválidas: {m}"),
            Self::Unavailable(m) => write!(f, "busca indisponível: {m}"),
        }
    }
}

/// Time spent in each mechanism of one retrieval.
#[derive(Default)]
struct Timings {
    semantic_ms: u64,
    lexical_ms: u64,
    candidates: u32,
}

pub struct HybridRetriever {
    /// The current embedding model; without one, retrieval is lexical only.
    embeddings: Arc<dyn EmbeddingSource>,
    vectors: Arc<dyn VectorStore>,
    lexical: Arc<dyn LexicalIndex>,
    chunks: Arc<dyn ChunkReader>,
    /// Vector index of the current provider (keyed by provider identity, so a model change re-resolves it).
    index: Mutex<Option<(usize, VectorIndexId)>>,
}

enum Outcome<T> {
    Skipped,
    Done(Vec<T>),
    Failed(String),
}

impl HybridRetriever {
    pub fn new(
        embeddings: Arc<dyn EmbeddingSource>,
        vectors: Arc<dyn VectorStore>,
        lexical: Arc<dyn LexicalIndex>,
        chunks: Arc<dyn ChunkReader>,
    ) -> Self {
        Self {
            embeddings,
            vectors,
            lexical,
            chunks,
            index: Mutex::new(None),
        }
    }

    async fn vector_index(
        &self,
        provider: &Arc<dyn EmbeddingProvider>,
    ) -> Result<VectorIndexId, String> {
        let key = Arc::as_ptr(provider) as *const () as usize;
        if let Some((cached, id)) = *self.index.lock().unwrap() {
            if cached == key {
                return Ok(id);
            }
        }
        let identity = provider.identity().await.map_err(|e| e.to_string())?;
        // The dimension comes from the loaded model.
        let index = self
            .vectors
            .create_index(&EmbeddingSpace::from_identity(&identity))
            .await
            .map_err(|e| e.to_string())?;
        *self.index.lock().unwrap() = Some((key, index.id));
        Ok(index.id)
    }

    async fn semantic(
        &self,
        query: &str,
        k: usize,
        filter: VectorFilter,
    ) -> Outcome<SemanticCandidate> {
        let Some(provider) = self.embeddings.current() else {
            return Outcome::Failed("nenhum modelo de embeddings configurado".into());
        };
        let run = async {
            let index = self.vector_index(&provider).await?;
            let vector = provider
                .embed(query, EmbeddingPurpose::Query)
                .await
                .map_err(|e| e.to_string())?;
            self.vectors
                .search(index, &vector, k, &filter)
                .await
                .map_err(|e| e.to_string())
        };
        match run.await {
            Ok(hits) => Outcome::Done(
                hits.into_iter()
                    .map(|h| SemanticCandidate {
                        chunk_id: h.chunk_id,
                        document_id: h.document_id,
                        similarity: h.similarity,
                    })
                    .collect(),
            ),
            Err(reason) => Outcome::Failed(reason),
        }
    }

    pub async fn retrieve(
        &self,
        query: &str,
        options: &RetrievalOptions,
    ) -> Result<RetrievalResult, RetrievalError> {
        let started = Instant::now();
        let mut timings = Timings::default();
        let result = self.run(query, options, &mut timings).await;
        match &result {
            Ok(r) => record(&Measurement::Retrieved {
                mode: match r.mode {
                    RetrievalMode::Hybrid => "hybrid",
                    RetrievalMode::SemanticOnly => "semantic",
                    RetrievalMode::LexicalOnly => "lexical",
                    RetrievalMode::Structure => "structure",
                },
                candidates: timings.candidates,
                passages: r.chunks.len() as u32,
                semantic_ms: timings.semantic_ms,
                lexical_ms: timings.lexical_ms,
                total_ms: ms(started),
            }),
            Err(e) => record(&Measurement::RetrieveFailed {
                kind: match e {
                    RetrievalError::InvalidOptions(_) => ErrorKind::Invalid,
                    RetrievalError::Unavailable(_) => ErrorKind::Unavailable,
                },
                total_ms: ms(started),
            }),
        }
        result
    }

    async fn run(
        &self,
        query: &str,
        options: &RetrievalOptions,
        timings: &mut Timings,
    ) -> Result<RetrievalResult, RetrievalError> {
        options
            .validate()
            .map_err(|e| RetrievalError::InvalidOptions(e.0))?;
        let query = normalize_query(query);
        if query.is_empty() {
            return Ok(RetrievalResult {
                chunks: Vec::new(),
                mode: RetrievalMode::Hybrid,
                warnings: Vec::new(),
            });
        }
        let k = options.candidates();
        let candidates = self
            .chunks
            .resolve(&options.filter)
            .await
            .map_err(|e| RetrievalError::Unavailable(e.message))?;

        let semantic = if options.semantic_weight > 0.0 {
            let filter = VectorFilter {
                documents: candidates.documents.clone(),
                chunks: candidates.chunks.clone(),
            };
            let started = Instant::now();
            let outcome = self.semantic(&query, k, filter).await;
            timings.semantic_ms = ms(started);
            outcome
        } else {
            Outcome::Skipped
        };
        let lexical_query = LexicalQuery::from_query(&query);
        let lexical: Outcome<LexicalCandidate> =
            if options.lexical_weight > 0.0 && !lexical_query.is_empty() {
                let started = Instant::now();
                let outcome = match self
                    .lexical
                    .search(&lexical_query, k, &options.filter)
                    .await
                {
                    Ok(hits) => Outcome::Done(hits),
                    Err(err) => Outcome::Failed(err.message),
                };
                timings.lexical_ms = ms(started);
                outcome
            } else {
                Outcome::Skipped
            };

        let mut warnings = Vec::new();
        let (semantic, semantic_weight) = match semantic {
            Outcome::Done(hits) => (Some(hits), options.semantic_weight),
            Outcome::Failed(reason) => {
                warnings.push(format!("busca semântica indisponível: {reason}"));
                (None, 0.0)
            }
            Outcome::Skipped => (None, 0.0),
        };
        let (lexical, lexical_weight) = match lexical {
            Outcome::Done(hits) => (Some(hits), options.lexical_weight),
            Outcome::Failed(reason) => {
                warnings.push(format!("busca textual indisponível: {reason}"));
                (None, 0.0)
            }
            Outcome::Skipped => (None, 0.0),
        };
        timings.candidates =
            (semantic.as_ref().map_or(0, Vec::len) + lexical.as_ref().map_or(0, Vec::len)) as u32;
        let mode = match (&semantic, &lexical) {
            (Some(_), Some(_)) => RetrievalMode::Hybrid,
            (Some(_), None) => RetrievalMode::SemanticOnly,
            (None, Some(_)) => RetrievalMode::LexicalOnly,
            (None, None) if warnings.is_empty() => {
                // Lexical had no usable terms and semantic is disabled by its weight.
                return Ok(RetrievalResult {
                    chunks: Vec::new(),
                    mode: RetrievalMode::LexicalOnly,
                    warnings,
                });
            }
            (None, None) => return Err(RetrievalError::Unavailable(warnings.join("; "))),
        };

        // Fraction of query terms each lexical candidate contains: a chunk matching one weak
        // term must not get a full lexical score just for being the best BM25 hit.
        let lexical_ids: Vec<i64> = lexical.iter().flatten().map(|c| c.chunk_id).collect();
        let coverage: HashMap<i64, f32> = if lexical_ids.is_empty() {
            HashMap::new()
        } else {
            self.chunks
                .get_many(&lexical_ids)
                .await
                .map_err(|e| RetrievalError::Unavailable(e.message))?
                .into_iter()
                .map(|v| (v.chunk_id, coverage(&v.text, &lexical_query.terms)))
                .collect()
        };
        let fused = fuse(
            semantic.as_deref().unwrap_or_default(),
            lexical.as_deref().unwrap_or_default(),
            &coverage,
            semantic_weight,
            lexical_weight,
            options.top_k,
        );
        let ids: Vec<i64> = fused.iter().map(|h| h.chunk_id).collect();
        let views = self
            .chunks
            .get_many(&ids)
            .await
            .map_err(|e| RetrievalError::Unavailable(e.message))?;
        let chunks = fused
            .into_iter()
            .filter_map(|hit| {
                let chunk = views.iter().find(|v| v.chunk_id == hit.chunk_id)?.clone();
                Some(RetrievedChunk {
                    chunk,
                    score: hit.score,
                    semantic: hit.semantic,
                    lexical: hit.lexical,
                })
            })
            .collect();
        Ok(RetrievalResult {
            chunks,
            mode,
            warnings,
        })
    }
}
