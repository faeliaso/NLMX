//! Section handlers. No business logic yet: each section renders its empty/initial state.

use askama::Template;
use axum::{
    extract::State,
    response::{IntoResponse, Response},
};
use http::HeaderMap;

use crate::{
    AppState,
    documents::{DocumentRow, Library},
    error::UiError,
    shell::{Section, page},
    status::{StorageView, VERSION},
};

#[derive(Template)]
#[template(path = "pages/sections/documents.html")]
struct DocumentsView {
    library: Library,
}

#[derive(Template)]
#[template(path = "components/document_list.html")]
struct DocumentListFragment {
    library: Library,
}

async fn library(state: &AppState) -> Library {
    match &state.ingestion {
        Err(reason) => Library::Unavailable(reason.clone()),
        Ok(ingestion) => match ingestion.list().await {
            Ok(docs) => Library::Documents(docs.into_iter().map(DocumentRow::from).collect()),
            Err(err) => Library::Unavailable(err.message),
        },
    }
}

#[derive(Template)]
#[template(path = "pages/sections/indexing.html")]
struct IndexingView;

#[derive(Template)]
#[template(path = "pages/sections/settings.html")]
struct SettingsView {
    version: &'static str,
    debug: bool,
    storage: StorageView,
    pdf_engine: Result<String, String>,
    runtime: Result<nlmx_domain::models::RuntimeInfo, String>,
    diagnostics: Option<crate::diagnostics::DiagnosticsView>,
    download_url: Option<&'static str>,
}

fn render(template: impl Template) -> Result<String, UiError> {
    Ok(template.render()?)
}

pub async fn documents(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let view = DocumentsView {
        library: library(&state).await,
    };
    page(&headers, Some(Section::Documents), render(view))
}

/// Re-rendered after an import (`documents-changed` event).
pub async fn documents_fragment(State(state): State<AppState>) -> Response {
    match render(DocumentListFragment {
        library: library(&state).await,
    }) {
        Ok(html) => axum::response::Html(html).into_response(),
        Err(err) => (err.status, err.message).into_response(),
    }
}

pub async fn indexing(headers: HeaderMap) -> Response {
    page(&headers, Some(Section::Indexing), render(IndexingView))
}

pub async fn settings(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let status = state.system_status.execute().await;
    let view = SettingsView {
        version: VERSION,
        debug: cfg!(debug_assertions),
        storage: StorageView::from(&status.storage),
        pdf_engine: status.document_engine,
        runtime: status.runtime,
        download_url: crate::DOWNLOAD_URL,
        diagnostics: match &state.diagnostics {
            Some(d) => {
                d.sample().await;
                Some(crate::diagnostics::DiagnosticsView::from(&d.snapshot()))
            }
            None => None,
        },
    };
    page(&headers, Some(Section::Settings), render(view))
}

pub async fn not_found(headers: HeaderMap) -> Response {
    page(&headers, None, Err(UiError::not_found()))
}
