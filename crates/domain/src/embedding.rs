//! Embedding concepts.

use std::fmt;

/// What a text is embedded for. Retrieval models use different instructions for each side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmbeddingPurpose {
    /// A user question (search side).
    Query,
    /// A document chunk (index side).
    Passage,
}

/// Identifies the model behind a vector space. Vectors from different identities never mix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelIdentity {
    /// Stable id from the configuration, e.g. "qwen3-embedding-0.6b-q8_0".
    pub id: String,
    pub file_name: String,
    pub file_size: u64,
    pub dimensions: u32,
    /// Context length the model was trained with.
    pub context_length: u32,
    pub parameters: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingError {
    /// The provider cannot run (missing binary/model, server failed to start).
    Unavailable(String),
    Timeout(String),
    InputTooLong(String),
    /// The server answered with an error.
    Server(String),
    /// The server answered something unexpected (wrong count, dimension, format).
    InvalidResponse(String),
}

impl fmt::Display for EmbeddingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "modelo de embeddings indisponível: {m}"),
            Self::Timeout(m) => write!(f, "tempo esgotado: {m}"),
            Self::InputTooLong(m) => write!(f, "texto longo demais para o modelo: {m}"),
            Self::Server(m) => write!(f, "erro do servidor de embeddings: {m}"),
            Self::InvalidResponse(m) => {
                write!(f, "resposta inválida do servidor de embeddings: {m}")
            }
        }
    }
}

impl std::error::Error for EmbeddingError {}
