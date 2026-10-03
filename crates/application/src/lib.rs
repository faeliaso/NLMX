//! Application layer: use cases, internal services (Document Engine, Retriever, RAG Engine)
//! and every port trait implemented by the adapters. See `docs/ARCHITECTURE.md` §3.

pub mod ports;
pub mod services;
pub mod telemetry;
pub mod use_cases;
