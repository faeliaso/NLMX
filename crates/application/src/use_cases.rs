//! Use cases exposed to the UI (`ImportDocuments`, `SearchChunks`, `AskQuestion`, ...).

mod chat;
mod embeddings;
mod indexing;
mod ingestion;
mod remove_document;
mod system_status;
mod viewer;

pub use chat::{ChatError, ChatService, describe as describe_model_status};
pub use embeddings::{EmbedDocuments, EmbedOutcome};
pub use indexing::{
    ActivityGuard, Indexing, IndexingActivity, IndexingError, IndexingReport, RetryOutcome,
};
pub use ingestion::{DocumentIngestion, Enqueued};
pub use remove_document::{RemoveDocument, RemoveError};
pub use system_status::{GetSystemStatus, SystemStatus};
pub use viewer::{
    DocumentOutline, MAX_SEARCH_HITS, MAX_WIDTH_PX, PageError, PageImage, PageSize, SearchHit,
    SearchResults, ViewDocument,
};
