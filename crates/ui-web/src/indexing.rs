//! Indexação: the state of the search index, the background work and what needs attention.

use askama::Template;
use axum::{
    extract::State,
    response::{Html, IntoResponse, Response},
};
use http::HeaderMap;
use nlmx_application::use_cases::IndexingReport;
use nlmx_domain::{
    indexing::IndexJob,
    ingestion::{DocumentId, DocumentStatus},
};
use nlmx_i18n::{Arg, t, t_args, t_count};

use crate::{
    AppState,
    documents::{date_label, status_badge},
    error::UiError,
    formats::format_view,
    shell::{Section, page as shell_page},
};

/// How many finished documents "Concluídos recentemente" lists.
const RECENT: usize = 5;

/// A button that runs a Tauri command.
pub struct Action {
    pub command: &'static str,
    /// The command's `id` argument.
    pub document_id: DocumentId,
    pub label: String,
}

pub struct JobRow {
    pub title: String,
    /// The format's icon and short label (see `formats`).
    pub icon: &'static str,
    pub format: &'static str,
    pub label: String,
    pub kind: &'static str,
    /// The error, or what the state means.
    pub detail: String,
    pub meta: String,
    pub action: Option<Action>,
}

pub struct Stat {
    pub label: String,
    pub value: String,
}

pub struct IndexingModel {
    /// Something changes without the user (the fragment polls).
    pub active: bool,
    /// Background work is running: the actions are disabled.
    pub running: bool,
    /// The active embedding model (`None`: lexical search only).
    pub model: Option<String>,
    pub stats: Vec<Stat>,
    pub attention: Vec<JobRow>,
    pub recent: Vec<JobRow>,
    pub embed_pending: bool,
    pub retry_failed: bool,
    pub reindex: bool,
    /// What reindexing redoes, for the confirmation dialog.
    pub reindex_description: String,
    pub empty: bool,
}

#[derive(Template)]
#[template(path = "pages/sections/indexing.html")]
struct IndexingPage {
    state: Result<IndexingModel, String>,
}

#[derive(Template)]
#[template(path = "components/indexing_body.html")]
struct IndexingBody {
    state: Result<IndexingModel, String>,
}

pub fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0 => t("indexing-duration-under-second"),
        1..60 => t_args(
            "indexing-duration-seconds",
            &[("seconds", Arg::Str(seconds.to_string()))],
        ),
        _ => t_args(
            "indexing-duration-minutes",
            &[
                ("minutes", Arg::Str((seconds / 60).to_string())),
                ("seconds", Arg::Str((seconds % 60).to_string())),
            ],
        ),
    }
}

fn chunks(count: u32) -> String {
    t_count("indexing-chunks-count", count as i64)
}

fn retry(id: DocumentId) -> Option<Action> {
    Some(Action {
        command: "retry_document",
        document_id: id,
        label: t("indexing-retry"),
    })
}

fn attempts(job: &IndexJob) -> String {
    t_count("indexing-attempts-count", job.attempts as i64)
}

impl IndexingModel {
    pub fn new(report: &IndexingReport, can_reread: bool) -> Self {
        let snapshot = &report.snapshot;
        let has_model = report.model_id.is_some();
        let (mut attention, mut done) = (Vec::new(), Vec::new());
        for job in &snapshot.jobs {
            let row = |label: String, kind, detail: String, meta: String, action| JobRow {
                title: job.title.clone(),
                icon: format_view(job.document_type).icon,
                format: format_view(job.document_type).label,
                label,
                kind,
                detail,
                meta,
                action,
            };
            match job.status {
                // Being read or embedded right now: not listed (the page refreshes itself, and the
                // document shows up below when it finishes or needs attention).
                DocumentStatus::Queued
                | DocumentStatus::Extracting
                | DocumentStatus::Structuring
                | DocumentStatus::Chunking => {}
                DocumentStatus::Embedding if has_model && report.running && job.error.is_none() => {
                }
                DocumentStatus::Embedding => {
                    let (detail, action) = match (&job.error, has_model) {
                        (Some(error), true) => (error.clone(), retry(job.document_id)),
                        (_, false) => (t("indexing-detail-waiting-model"), None),
                        (None, true) => {
                            (t("indexing-detail-no-embeddings"), retry(job.document_id))
                        }
                    };
                    let (label, kind) = status_badge(job.status);
                    attention.push(row(label, kind, detail, chunks(job.chunks), action));
                }
                DocumentStatus::Failed => {
                    let (label, kind) = status_badge(job.status);
                    attention.push(row(
                        label,
                        kind,
                        job.error.clone().unwrap_or_default(),
                        attempts(job),
                        if can_reread {
                            retry(job.document_id)
                        } else {
                            None
                        },
                    ));
                }
                DocumentStatus::NeedsOcr => {
                    let (label, kind) = status_badge(job.status);
                    attention.push(row(
                        label,
                        kind,
                        t("indexing-detail-needs-ocr"),
                        String::new(),
                        None,
                    ));
                }
                DocumentStatus::Indexed => done.push(job),
            }
        }
        // Most recently finished first; documents indexed before jobs had timestamps go last.
        done.sort_by(|a, b| b.finished_at.cmp(&a.finished_at));
        let recent = done
            .iter()
            .take(RECENT)
            .map(|job| {
                let mut meta = Vec::new();
                if let Some(at) = &job.finished_at {
                    meta.push(date_label(at));
                }
                if let Some(ms) = job.duration_ms {
                    meta.push(duration(ms));
                }
                meta.push(chunks(job.chunks));
                let (label, kind) = status_badge(job.status);
                JobRow {
                    title: job.title.clone(),
                    icon: format_view(job.document_type).icon,
                    format: format_view(job.document_type).label,
                    label,
                    kind,
                    detail: String::new(),
                    meta: meta.join(" · "),
                    action: None,
                }
            })
            .collect();

        let indexed = snapshot.count(DocumentStatus::Indexed);
        let failed = snapshot.count(DocumentStatus::Failed);
        let mut documents = vec![t_count("indexing-docs-indexed", indexed as i64)];
        let reading = snapshot.reading();
        let awaiting = snapshot.count(DocumentStatus::Embedding);
        let no_text = snapshot.count(DocumentStatus::NeedsOcr);
        if reading > 0 {
            documents.push(t_count("indexing-docs-reading", reading as i64));
        }
        if awaiting > 0 {
            documents.push(t_count("indexing-docs-awaiting", awaiting as i64));
        }
        if no_text > 0 {
            documents.push(t_count("indexing-docs-no-text", no_text as i64));
        }
        if failed > 0 {
            documents.push(t_count("indexing-docs-failed", failed as i64));
        }
        let mut chunk_stats = vec![t_count("indexing-chunks-total", snapshot.chunks as i64)];
        if has_model {
            chunk_stats.push(t_count(
                "indexing-chunks-embedded",
                snapshot.embedded_chunks() as i64,
            ));
        }
        if snapshot.pending_chunks() > 0 {
            chunk_stats.push(t_count(
                "indexing-chunks-pending",
                snapshot.pending_chunks() as i64,
            ));
        }
        let mut stats = vec![
            Stat {
                label: t("indexing-stat-documents"),
                value: documents.join(" · "),
            },
            Stat {
                label: t("indexing-stat-chunks"),
                value: chunk_stats.join(" · "),
            },
        ];
        if let Some(model) = &snapshot.model {
            stats.push(Stat {
                label: t("indexing-stat-dimensions"),
                value: model.dimensions.to_string(),
            });
        }
        let indexed_chunks: u32 = snapshot
            .jobs
            .iter()
            .filter(|j| j.status == DocumentStatus::Indexed)
            .map(|j| j.chunks)
            .sum();
        Self {
            active: report.active(),
            running: report.running,
            model: report.model_id.clone(),
            stats,
            attention,
            recent,
            embed_pending: has_model && snapshot.count(DocumentStatus::Embedding) > 0,
            retry_failed: can_reread && failed > 0,
            reindex: has_model && indexed > 0,
            reindex_description: t_args(
                "indexing-reindex-description",
                &[
                    (
                        "documents",
                        Arg::Str(t_count("indexing-documents-count", indexed as i64)),
                    ),
                    ("chunks", Arg::Str(chunks(indexed_chunks))),
                ],
            ),
            empty: snapshot.jobs.is_empty(),
        }
    }
}

async fn state(app: &AppState) -> Result<IndexingModel, String> {
    let indexing = app.indexing.as_ref().map_err(Clone::clone)?;
    let report = indexing.report().await.map_err(|e| e.message)?;
    Ok(IndexingModel::new(&report, indexing.ingestion.is_ok()))
}

pub async fn page(State(app): State<AppState>, headers: HeaderMap) -> Response {
    let view = IndexingPage {
        state: state(&app).await,
    };
    shell_page(
        &headers,
        Some(Section::Indexing),
        view.render().map_err(UiError::from),
    )
}

/// Re-rendered while work is running and after the actions (`indexing-changed` event).
pub async fn fragment(State(app): State<AppState>) -> Response {
    match (IndexingBody {
        state: state(&app).await,
    })
    .render()
    {
        Ok(html) => Html(html).into_response(),
        Err(err) => {
            let err = UiError::from(err);
            (err.status, err.message).into_response()
        }
    }
}
