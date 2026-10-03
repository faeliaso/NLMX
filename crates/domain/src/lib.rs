//! Domain layer: entities (`Document`, `Chunk`, `EmbeddingSpace`, ...), value objects
//! and pure policies (RRF, chunk policy). Must not depend on any other workspace crate
//! or on infrastructure libraries. See `docs/ARCHITECTURE.md` §1.

pub mod chat;
pub mod document;
pub mod embedding;
pub mod generation;
pub mod ingestion;
pub mod models;
pub mod rag_intent;
pub mod retrieval;
pub mod telemetry;
pub mod vectors;
pub mod viewer;
