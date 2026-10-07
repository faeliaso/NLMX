//! Source information: what the interface shows about a source of any format — what it is and
//! how it was indexed — in the side panel of the chat. Only a PDF has a preview (the viewer);
//! for every other format this is all there is, and nothing here shows the file's content.
//!
//! `GET /sources/{id}[?cite={message}-{n}]`; with `cite`, also the passage of that answer.

use askama::Template;
use axum::{
    extract::{Path, Query, State},
    response::{Html, IntoResponse, Response},
};
use nlmx_domain::ingestion::DocumentId;
use nlmx_i18n::{current, format_bytes, t, t_args, t_count};
use serde::Deserialize;

use crate::{
    AppState,
    documents::{date_label, status_badge},
    error::UiError,
    formats::{format_description, format_view},
    markdown::escape,
    viewer::pair,
};

/// Longest cited passage shown, in characters.
const QUOTE_CHARS: usize = 700;

pub struct CitedView {
    /// "Trecho citado · p. 3".
    pub label: String,
    pub quote: String,
}

#[derive(Template)]
#[template(path = "components/source_info.html")]
pub struct SourceInfoView {
    pub id: DocumentId,
    pub icon: &'static str,
    pub format: &'static str,
    /// What the format is called in full ("Planilha do Microsoft Excel").
    pub description: String,
    pub title: String,
    /// A note has no file name or size.
    pub is_note: bool,
    pub file_name: String,
    pub status_label: String,
    pub status_kind: &'static str,
    pub error: String,
    pub chunks: String,
    pub size: String,
    pub pages: Option<String>,
    /// Worksheets of a workbook.
    pub sheets: Option<String>,
    pub imported_on: String,
    pub indexed_on: Option<String>,
    pub used_in: String,
    /// A PDF can also be opened in the viewer.
    pub previewable: bool,
    /// "Fonte 2", when it was opened from an answer.
    pub origin: String,
    pub cited: Option<CitedView>,
}

#[derive(Deserialize, Default)]
pub struct SourceQuery {
    /// `{message}-{n}`: a source of an answer.
    pub cite: Option<String>,
}

/// The passage of an answer's source `n` in `document`, when the request came from a citation.
async fn cited(
    state: &AppState,
    document: DocumentId,
    cite: Option<&str>,
) -> Option<(u32, CitedView)> {
    let (message, n) = pair(cite)?;
    let message = state.chat.as_ref().ok()?.message(message).await.ok()?;
    let source = message
        .sources
        .iter()
        .find(|s| s.n == n && s.document_id == document)?;
    let mut quote: String = source.quote.chars().take(QUOTE_CHARS).collect();
    if source.quote.chars().count() > QUOTE_CHARS {
        quote.push('…');
    }
    Some((
        n,
        CitedView {
            label: t_args(
                "sources-cited-label",
                &[("location", source.reference.location.label().into())],
            ),
            quote,
        },
    ))
}

/// The panel of a source (also used by the chat page when it opens with `?source=`).
pub async fn render(
    state: &AppState,
    id: DocumentId,
    query: &SourceQuery,
) -> Result<String, UiError> {
    let ingestion = state.ingestion.as_ref().map_err(UiError::internal)?;
    let details = ingestion
        .documents
        .source_details(id)
        .await
        .map_err(|e| UiError::internal(e.message))?
        .ok_or_else(UiError::not_found)?;
    let (status_label, status_kind) = status_badge(details.status);
    let format = format_view(details.document_type);
    let from_answer = cited(state, id, query.cite.as_deref()).await;
    let view = SourceInfoView {
        id,
        icon: format.icon,
        format: format.label,
        description: format_description(details.document_type),
        title: if details.document_type.is_note() {
            details.title.clone()
        } else {
            details.file_name.clone()
        },
        is_note: details.document_type.is_note(),
        file_name: details.file_name,
        status_label,
        status_kind,
        error: details.error.unwrap_or_default(),
        chunks: t_count("sources-chunk-count", i64::from(details.chunks)),
        size: format_bytes(current(), details.file_size),
        pages: details
            .page_count
            .map(|n| t_count("sources-page-count", i64::from(n))),
        sheets: details
            .sheets
            .map(|n| t_count("sources-sheet-count", i64::from(n))),
        imported_on: date_label(&details.imported_at),
        indexed_on: details.indexed_at.as_deref().map(date_label),
        used_in: match details.conversations {
            0 => t("sources-not-used"),
            n => t_count("sources-conversation-count", i64::from(n)),
        },
        previewable: details.document_type.previewable(),
        origin: from_answer.as_ref().map_or_else(String::new, |(n, _)| {
            t_args("sources-origin", &[("n", u64::from(*n).into())])
        }),
        cited: from_answer.map(|(_, c)| c),
    };
    Ok(view.render()?)
}

fn error_fragment(e: UiError) -> Response {
    let html = format!(
        r#"<div class="viewer-error"><div class="alert alert-danger" role="alert"><div class="alert-content"><p class="alert-title">{}</p><p>{}</p></div></div><button type="button" class="btn btn-secondary btn-sm" data-close-panel>{}</button></div>"#,
        escape(&e.title),
        escape(&e.message),
        escape(&t("common-close"))
    );
    (e.status, Html(html)).into_response()
}

/// `GET /sources/{id}`.
pub async fn open(
    State(state): State<AppState>,
    Path(id): Path<DocumentId>,
    Query(query): Query<SourceQuery>,
) -> Response {
    match render(&state, id, &query).await {
        Ok(html) => Html(html).into_response(),
        Err(e) => error_fragment(e),
    }
}
