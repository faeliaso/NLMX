//! PDF viewer: the document opened beside the chat (or alone), with its pages rendered by
//! PDFium, a selectable text layer, search and the highlights of a citation.
//!
//! `GET /viewer/{doc}?page=N&cite={message}-{n}` (a source) or `&ref={message}-{page}`
//! (a `[página N]` reference) opens the document at the page, positioned on the passage.

use std::sync::Arc;

use askama::Template;
use axum::{
    Json,
    extract::{Path, Query, State},
    response::{Html, IntoResponse, Response},
};
use http::{StatusCode, header};
use nlmx_application::use_cases::{DocumentOutline, PageError, ViewDocument};
use nlmx_domain::{
    document::BoundingBox, ingestion::DocumentId, ingestion::PageBox, viewer::ViewerTarget,
};
use serde::{Deserialize, Serialize};

use crate::{AppState, error::UiError, markdown::escape};

/// A box as percentages of its page (what the overlays use).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PercentBox {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

pub fn percent(b: &BoundingBox, width: f32, height: f32) -> PercentBox {
    let pct = |v: f32, total: f32| round2((v / total.max(1.0) * 100.0).clamp(0.0, 100.0));
    PercentBox {
        left: pct(b.left, width),
        top: pct(b.top, height),
        width: pct(b.right - b.left, width),
        height: pct(b.bottom - b.top, height),
    }
}

pub struct PageView {
    pub number: u32,
    pub width: f32,
    pub height: f32,
    pub highlights: Vec<PercentBox>,
}

#[derive(Template)]
#[template(path = "components/viewer.html")]
pub struct ViewerView {
    pub document_id: DocumentId,
    pub title: String,
    pub pages: Vec<PageView>,
    pub page: u32,
    /// Where to scroll on the target page, in % of its height (top of the first highlight).
    pub anchor: String,
    pub has_highlights: bool,
    /// What opened it, for the toolbar ("Fonte 3").
    pub origin: String,
}

impl ViewerView {
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
}

/// The viewer for `target` (highlights clipped to each page).
pub fn view(outline: DocumentOutline, target: &ViewerTarget, origin: String) -> ViewerView {
    let page = target.page.clamp(1, outline.page_count().max(1));
    let pages: Vec<PageView> = outline
        .pages
        .iter()
        .map(|p| PageView {
            number: p.number,
            width: p.width,
            height: p.height,
            highlights: target
                .highlights
                .iter()
                .filter(|b| b.page == p.number)
                .map(|b| percent(&b.bbox, p.width, p.height))
                .collect(),
        })
        .collect();
    let anchor = pages
        .iter()
        .find(|p| p.number == page)
        .and_then(|p| p.highlights.iter().map(|h| h.top).min_by(f32::total_cmp))
        .map(|t| t.to_string())
        .unwrap_or_default();
    ViewerView {
        document_id: outline.document_id,
        title: outline.title,
        has_highlights: !target.highlights.is_empty(),
        pages,
        page,
        anchor,
        origin,
    }
}

fn viewer_service(state: &AppState) -> Result<&Arc<ViewDocument>, UiError> {
    state.viewer.as_ref().map_err(|reason| UiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        title: "Visualizador indisponível".into(),
        message: reason.clone(),
    })
}

fn page_error(e: PageError) -> UiError {
    match e {
        PageError::NotFound => UiError::not_found(),
        PageError::Failed(m) => UiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            title: "Não foi possível abrir o documento".into(),
            message: m,
        },
    }
}

fn error_fragment(e: UiError) -> Response {
    let html = format!(
        r#"<div class="viewer-error"><div class="alert alert-danger" role="alert"><div class="alert-content"><p class="alert-title">{}</p><p>{}</p></div></div><button type="button" class="btn btn-secondary btn-sm" data-close-panel>Fechar</button></div>"#,
        escape(&e.title),
        escape(&e.message)
    );
    (e.status, Html(html)).into_response()
}

#[derive(Deserialize, Default)]
pub struct ViewerQuery {
    pub page: Option<u32>,
    /// `{message}-{n}`: a source of an answer.
    pub cite: Option<String>,
    /// `{message}-{page}`: a `[página N]` reference of an answer.
    #[serde(rename = "ref")]
    pub page_ref: Option<String>,
}

fn pair(value: Option<&str>) -> Option<(i64, u32)> {
    let (a, b) = value?.split_once('-')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// The target of a viewer request: the source or page reference it came from, if any.
async fn target(
    state: &AppState,
    document: DocumentId,
    query: &ViewerQuery,
) -> (ViewerTarget, String) {
    let plain = ViewerTarget {
        document_id: document,
        page: query.page.unwrap_or(1),
        highlights: Vec::new(),
    };
    let Ok(chat) = &state.chat else {
        return (plain, String::new());
    };
    if let Some((message, n)) = pair(query.cite.as_deref()) {
        if let Ok(m) = chat.message(message).await {
            if let Some(s) = m
                .sources
                .iter()
                .find(|s| s.n == n && s.document_id == document)
            {
                return (
                    ViewerTarget {
                        document_id: document,
                        page: query.page.unwrap_or(s.page_start),
                        highlights: s.bboxes.clone(),
                    },
                    format!("Fonte {n}"),
                );
            }
        }
    }
    if let Some((message, page)) = pair(query.page_ref.as_deref()) {
        if let Ok(m) = chat.message(message).await {
            if let Some(r) = m
                .page_refs
                .iter()
                .find(|r| r.page == page && r.document_id == document)
            {
                let highlights: Vec<PageBox> = r
                    .source
                    .and_then(|n| m.sources.iter().find(|s| s.n == n))
                    .map(|s| {
                        s.bboxes
                            .iter()
                            .filter(|b| b.page == page)
                            .copied()
                            .collect()
                    })
                    .unwrap_or_default();
                return (
                    ViewerTarget {
                        document_id: document,
                        page,
                        highlights,
                    },
                    format!("Página {page}"),
                );
            }
        }
    }
    (plain, String::new())
}

/// Renders the viewer (also used by the chat page when it opens with `?view=`).
pub async fn render(
    state: &AppState,
    document: DocumentId,
    query: &ViewerQuery,
) -> Result<String, UiError> {
    let viewer = viewer_service(state)?;
    let outline = viewer.outline(document).await.map_err(page_error)?;
    let (target, origin) = target(state, document, query).await;
    Ok(view(outline, &target, origin).render()?)
}

/// `GET /viewer/{doc}`.
pub async fn open(
    State(state): State<AppState>,
    Path(document): Path<DocumentId>,
    Query(query): Query<ViewerQuery>,
) -> Response {
    match render(&state, document, &query).await {
        Ok(html) => Html(html).into_response(),
        Err(e) => error_fragment(e),
    }
}

pub struct TextSpanView {
    pub text: String,
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Template)]
#[template(path = "components/viewer_text.html")]
struct TextLayer {
    spans: Vec<TextSpanView>,
}

/// `GET /viewer/{doc}/pages/{n}/text`: the selectable text layer of a page.
pub async fn text(
    State(state): State<AppState>,
    Path((document, page)): Path<(DocumentId, u32)>,
) -> Response {
    let result = async {
        let viewer = viewer_service(&state)?;
        let outline = viewer.outline(document).await.map_err(page_error)?;
        let size = outline
            .pages
            .iter()
            .find(|p| p.number == page)
            .ok_or_else(UiError::not_found)?;
        let spans = viewer
            .text_layer(document, page)
            .await
            .map_err(page_error)?;
        Ok::<_, UiError>(TextLayer {
            spans: spans
                .iter()
                .filter(|s| !s.text.trim().is_empty())
                .map(|s| {
                    let b = percent(&s.bbox, size.width, size.height);
                    TextSpanView {
                        text: s.text.clone(),
                        left: b.left,
                        top: b.top,
                        width: b.width,
                        height: b.height,
                    }
                })
                .collect(),
        })
    }
    .await;
    match result.and_then(|t| Ok(t.render()?)) {
        Ok(html) => Html(html).into_response(),
        Err(e) => (e.status, e.message).into_response(),
    }
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: String,
}

#[derive(Serialize)]
struct HitJson {
    page: u32,
    boxes: Vec<PercentBox>,
}

#[derive(Serialize)]
struct SearchJson {
    hits: Vec<HitJson>,
    truncated: bool,
}

/// `GET /viewer/{doc}/search?q=`: matches with their boxes (in % of each page).
pub async fn search(
    State(state): State<AppState>,
    Path(document): Path<DocumentId>,
    Query(query): Query<SearchQuery>,
) -> Response {
    let result = async {
        let viewer = viewer_service(&state)?;
        let outline = viewer.outline(document).await.map_err(page_error)?;
        let results = viewer
            .search(document, &query.q)
            .await
            .map_err(page_error)?;
        let size = |page: u32| {
            outline
                .pages
                .iter()
                .find(|p| p.number == page)
                .map_or((612.0, 792.0), |p| (p.width, p.height))
        };
        Ok::<_, UiError>(SearchJson {
            truncated: results.truncated,
            hits: results
                .hits
                .into_iter()
                .map(|h| {
                    let (w, ht) = size(h.page);
                    HitJson {
                        page: h.page,
                        boxes: h.boxes.iter().map(|b| percent(b, w, ht)).collect(),
                    }
                })
                .collect(),
        })
    }
    .await;
    match result {
        Ok(json) => Json(json).into_response(),
        Err(e) => (e.status, e.message).into_response(),
    }
}

#[derive(Deserialize)]
pub struct ImageQuery {
    w: Option<u32>,
}

/// `GET /documents/{id}/pages/{n}.png?w=900`.
pub async fn page_image(
    State(state): State<AppState>,
    Path((document, file)): Path<(i64, String)>,
    Query(query): Query<ImageQuery>,
) -> Response {
    let Some(number) = file
        .strip_suffix(".png")
        .and_then(|n| n.parse::<u32>().ok())
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(viewer) = &state.viewer else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    match viewer
        .render(document, number, query.w.unwrap_or(900))
        .await
    {
        Ok(image) => (
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "private, max-age=3600"),
            ],
            image.png,
        )
            .into_response(),
        Err(PageError::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
