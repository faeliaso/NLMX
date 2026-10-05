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
use serde::Deserialize;

use crate::{
    AppState,
    documents::{date_label, status_badge},
    error::UiError,
    formats::format_view,
    markdown::escape,
    viewer::pair,
};

/// Longest cited passage shown, in characters.
const QUOTE_CHARS: usize = 700;

pub struct CitedView {
    pub location: String,
    pub quote: String,
}

#[derive(Template)]
#[template(path = "components/source_info.html")]
pub struct SourceInfoView {
    pub id: DocumentId,
    pub icon: &'static str,
    pub format: &'static str,
    /// What the format is called in full ("Planilha do Microsoft Excel").
    pub description: &'static str,
    pub title: String,
    /// A note has no file name or size.
    pub is_note: bool,
    pub file_name: String,
    pub status_label: &'static str,
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

fn size_label(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{:.1} MB", b / (KB * KB)).replace('.', ",")
    }
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
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
            location: source.reference.location.label(),
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
        description: format.description,
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
        chunks: plural(details.chunks, "trecho", "trechos"),
        size: size_label(details.file_size),
        pages: details.page_count.map(|n| plural(n, "página", "páginas")),
        sheets: details.sheets.map(|n| plural(n, "planilha", "planilhas")),
        imported_on: date_label(&details.imported_at),
        indexed_on: details.indexed_at.as_deref().map(date_label),
        used_in: match details.conversations {
            0 => "Ainda não usado em conversas".to_string(),
            n => plural(n, "conversa", "conversas"),
        },
        previewable: details.document_type.previewable(),
        origin: from_answer
            .as_ref()
            .map_or_else(String::new, |(n, _)| format!("Fonte {n}")),
        cited: from_answer.map(|(_, c)| c),
    };
    Ok(view.render()?)
}

fn error_fragment(e: UiError) -> Response {
    let html = format!(
        r#"<div class="viewer-error"><div class="alert alert-danger" role="alert"><div class="alert-content"><p class="alert-title">{}</p><p>{}</p></div></div><button type="button" class="btn btn-secondary btn-sm" data-close-panel>Fechar</button></div>"#,
        escape(&e.title),
        escape(&e.message)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_readable() {
        assert_eq!(size_label(512), "512 B");
        assert_eq!(size_label(2048), "2 KB");
        assert_eq!(size_label(3 * 1024 * 1024 + 512 * 1024), "3,5 MB");
    }
}
