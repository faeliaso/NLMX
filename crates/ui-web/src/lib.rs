//! Presentation layer. Exposes an axum [`Router`] that is driven in-process by the Tauri
//! custom URI scheme — there is no TCP server (ADR 0003). Handlers call application use
//! cases and return full pages or HTMX fragments rendered with askama.

mod assets;
mod chat;
mod diagnostics;
mod documents;
mod error;
mod fm_setup;
mod formats;
#[cfg(debug_assertions)]
mod gallery;
mod highlight;
mod indexing;
mod js_catalog;
mod language;
pub use language::LanguageSettings;
mod markdown;
pub use markdown::is_safe_url;
mod models;
mod sections;
mod shell;
mod sources;
mod status;
mod token_usage;
mod viewer;

use std::sync::Arc;

use axum::routing::post;
use axum::{Router, http::HeaderValue, middleware, response::Response, routing::get};
use nlmx_application::{
    ports::{Diagnostics, ModelProvider, NoteSubmitter},
    use_cases::{
        ChatService, DocumentIngestion, GetSystemStatus, Indexing, RemoveDocument, ViewDocument,
    },
};

pub use error::fallback_error_html;

/// Where new versions are published (set at build time; no automatic update check).
pub const DOWNLOAD_URL: Option<&str> = option_env!("NLMX_DOWNLOAD_URL");

const CONTENT_SECURITY_POLICY: &str =
    "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:";

/// Use cases the UI depends on, injected by the composition root.
#[derive(Clone)]
pub struct AppState {
    pub system_status: Arc<GetSystemStatus>,
    /// Unavailable (with the reason) when the database or the PDF engine failed to load.
    pub ingestion: Result<Arc<DocumentIngestion>, String>,
    /// Conversations answered from the documents (unavailable without the database).
    pub chat: Result<Arc<ChatService>, String>,
    /// The PDF viewer (unavailable without the database or the PDF engine).
    pub viewer: Result<Arc<ViewDocument>, String>,
    /// Removes documents with everything derived from them (unavailable without the database).
    pub remover: Result<Arc<RemoveDocument>, String>,
    /// Local session measurements (`None` hides the diagnostics section).
    pub diagnostics: Option<Arc<dyn Diagnostics>>,
    /// Embedding model files (catalog, downloads); `None` hides the model list.
    pub models: Option<Arc<dyn ModelProvider>>,
    /// The state of the index and the background work (unavailable without the database).
    pub indexing: Result<Arc<Indexing>, String>,
    /// Adds pasted notes to the import queue (`None` hides "Adicionar nota").
    pub notes: Option<Arc<dyn NoteSubmitter>>,
    /// Interface language: saved choice, macOS language, switch at runtime.
    pub language: LanguageSettings,
}

/// Builds the UI router served under the app's custom scheme.
pub fn router(state: AppState) -> Router {
    let router = Router::new()
        // The app starts directly in Chat.
        .route("/", get(chat::current))
        .route("/chat", get(chat::current))
        .route("/chat/new", post(chat::new))
        .route("/chat/{id}", get(chat::conversation))
        .route("/chat/{id}/scope", post(chat::scope))
        .route("/chat/{id}/delete", post(chat::delete))
        .route("/chat/{id}/messages", post(chat::ask))
        .route("/chat/messages/{id}", get(chat::answer))
        .route("/chat/messages/{id}/regenerate", post(chat::regenerate))
        .route("/chat/messages/{id}/free", post(chat::answer_freely))
        .route("/viewer/{doc}", get(viewer::open))
        .route("/sources/{id}", get(sources::open))
        .route("/viewer/{doc}/pages/{page}/text", get(viewer::text))
        .route("/viewer/{doc}/search", get(viewer::search))
        .route("/documents/{id}/pages/{file}", get(viewer::page_image))
        .route("/documents", get(sections::documents))
        .route("/documents/notes", post(sections::add_note))
        .route("/documents/{id}/delete", post(sections::remove_document))
        .route("/indexing", get(indexing::page))
        .route("/models", get(models::page))
        .route("/settings", get(sections::settings))
        .route("/settings/language", post(language::set))
        .route("/fragments/status", get(status::fragment))
        .route("/fragments/token-usage", get(token_usage::fragment))
        .route("/fragments/fm-setup", get(fm_setup::fragment))
        .route("/fragments/fm-setup/recheck", post(fm_setup::recheck))
        .route("/fragments/documents", get(sections::documents_fragment))
        .route("/fragments/models", get(models::fragment))
        .route("/fragments/indexing", get(indexing::fragment))
        .route("/assets/app.css", get(assets::app_css))
        .route("/assets/htmx.min.js", get(assets::htmx_js))
        .route("/assets/ds.js", get(assets::ds_js))
        .route("/assets/app.js", get(assets::app_js))
        .route("/assets/viewer.js", get(assets::viewer_js));

    #[cfg(debug_assertions)]
    let router = router.route("/design-system", get(gallery::page));

    router
        .fallback(sections::not_found)
        .layer(middleware::map_response(add_security_headers))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            language::load_before_request,
        ))
        .with_state(state)
}

async fn add_security_headers(mut response: Response) -> Response {
    response.headers_mut().insert(
        http::header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    response
}
