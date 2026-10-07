//! Section handlers. No business logic yet: each section renders its empty/initial state.

use askama::Template;
use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
};
use http::HeaderMap;

use crate::{
    AppState,
    documents::{DocumentRow, Library, Notice, removal_description},
    error::UiError,
    shell::{Section, page},
    status::{StorageView, VERSION},
};

#[derive(Template)]
#[template(path = "pages/sections/documents.html")]
struct DocumentsView {
    library: Library,
    notice: Option<Notice>,
}

#[derive(Template)]
#[template(path = "components/document_list.html")]
struct DocumentListFragment {
    library: Library,
}

async fn library(state: &AppState) -> Library {
    let docs = match &state.ingestion {
        Err(reason) => return Library::Unavailable(reason.clone()),
        Ok(ingestion) => match ingestion.list().await {
            Ok(docs) => docs,
            Err(err) => return Library::Unavailable(err.message),
        },
    };
    let mut rows = Vec::with_capacity(docs.len());
    for doc in docs {
        let mut row = DocumentRow::from(doc);
        match &state.remover {
            Err(_) => row.removable = false,
            // The dialog says how much of the chat history goes with the document.
            Ok(remover) if row.removable => {
                if let Ok(impact) = remover.impact(row.id).await {
                    row.removal = removal_description(&impact);
                }
            }
            Ok(_) => {}
        }
        rows.push(row);
    }
    Library::Documents(rows)
}

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
    languages: Vec<(&'static str, &'static str, bool)>,
}

fn remove_failed(reason: &dyn std::fmt::Display) -> String {
    nlmx_i18n::t_args(
        "documents-notice-remove-failed",
        &[("reason", reason.to_string().into())],
    )
}

fn render(template: impl Template) -> Result<String, UiError> {
    Ok(template.render()?)
}

pub async fn documents(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let view = DocumentsView {
        library: library(&state).await,
        notice: None,
    };
    page(&headers, Some(Section::Documents), render(view))
}

/// `POST /documents/{id}/delete`: removes the document (confirmed in a dialog) and shows the
/// library again, announcing the outcome as a toast.
pub async fn remove_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let notice = match &state.remover {
        Err(reason) => Notice {
            kind: "danger",
            message: remove_failed(reason),
        },
        Ok(remover) => match remover.remove(id).await {
            Ok(_) => Notice {
                kind: "success",
                message: nlmx_i18n::t("documents-notice-removed"),
            },
            Err(err) => Notice {
                kind: "danger",
                message: remove_failed(&err),
            },
        },
    };
    let view = DocumentsView {
        library: library(&state).await,
        notice: Some(notice),
    };
    page(&headers, Some(Section::Documents), render(view))
}

#[derive(serde::Deserialize)]
pub struct NoteForm {
    #[serde(default)]
    text: String,
}

/// `POST /documents/notes`: the text pasted in the "Adicionar nota" dialog goes to the import
/// queue like any other source (it shows in the library as `queued` and is indexed in the
/// background). The outcome of the submission is announced as a toast.
pub async fn add_note(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Form(form): axum::Form<NoteForm>,
) -> Response {
    let notice = match &state.notes {
        None => Notice {
            kind: "danger",
            message: nlmx_i18n::t("documents-notice-note-unavailable"),
        },
        Some(notes) => match notes.submit(&form.text) {
            Ok(()) => Notice {
                kind: "info",
                message: nlmx_i18n::t("documents-notice-note-added"),
            },
            Err(err) => Notice {
                kind: "danger",
                message: err.to_string(),
            },
        },
    };
    let view = DocumentsView {
        library: library(&state).await,
        notice: Some(notice),
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

pub async fn settings(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let status = state.system_status.execute().await;
    let view = SettingsView {
        version: VERSION,
        debug: cfg!(debug_assertions),
        storage: StorageView::from(&status.storage),
        pdf_engine: status.document_engine,
        runtime: status.runtime,
        download_url: crate::DOWNLOAD_URL,
        languages: crate::language::options(),
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
