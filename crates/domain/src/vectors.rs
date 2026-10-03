//! Vector search concepts. Dimensions always come from the embedding model, never from code.

use std::fmt;

use crate::{embedding::ModelIdentity, ingestion::DocumentId};

pub type ChunkId = i64;
pub type VectorIndexId = i64;

/// The vector space of one embedding model. Vectors from different spaces are never compared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingSpace {
    pub model_id: String,
    /// Distinguishes builds of the same model id (here: GGUF file name and size).
    pub revision: String,
    pub dimensions: u32,
    /// Context length of the model (informational; stored with the index).
    pub context_length: u32,
}

impl EmbeddingSpace {
    /// The space of the loaded model, with the dimension it reports.
    pub fn from_identity(identity: &ModelIdentity) -> Self {
        Self {
            model_id: identity.id.clone(),
            revision: format!("{}:{}", identity.file_name, identity.file_size),
            dimensions: identity.dimensions,
            context_length: identity.context_length,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VectorIndex {
    pub id: VectorIndexId,
    pub space: EmbeddingSpace,
    /// Storage table name (informational).
    pub table: String,
    pub count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VectorHit {
    pub chunk_id: ChunkId,
    pub document_id: DocumentId,
    /// Cosine distance (0 = same direction, 2 = opposite).
    pub distance: f32,
    /// `1 − distance`, i.e. cosine similarity.
    pub similarity: f32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VectorFilter {
    /// Restrict results to these documents (`None` = all).
    pub documents: Option<Vec<DocumentId>>,
    /// Restrict results to these chunks (`None` = all), e.g. resolved from a page filter.
    pub chunks: Option<Vec<ChunkId>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeleteScope {
    Chunks(Vec<ChunkId>),
    Document(DocumentId),
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VectorError {
    DimensionMismatch {
        expected: u32,
        actual: u32,
    },
    /// Empty vector or non-finite values (NaN/∞).
    InvalidVector(String),
    UnknownIndex(VectorIndexId),
    UnknownChunk(ChunkId),
    Storage(String),
}

impl fmt::Display for VectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DimensionMismatch { expected, actual } => {
                write!(
                    f,
                    "vetor com {actual} dimensões, o índice espera {expected}"
                )
            }
            Self::InvalidVector(m) => write!(f, "vetor inválido: {m}"),
            Self::UnknownIndex(id) => write!(f, "índice vetorial {id} não existe"),
            Self::UnknownChunk(id) => write!(f, "trecho {id} não existe"),
            Self::Storage(m) => write!(f, "erro no armazenamento de vetores: {m}"),
        }
    }
}

impl std::error::Error for VectorError {}
