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
    pub label: &'static str,
}

pub struct JobRow {
    pub title: String,
    /// The format's icon and short label (see `formats`).
    pub icon: &'static str,
    pub format: &'static str,
    pub label: &'static str,
    pub kind: &'static str,
    /// The error, or what the state means.
    pub detail: String,
    pub meta: String,
    pub action: Option<Action>,
}

pub struct Stat {
    pub label: &'static str,
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

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0 => "menos de 1 s".into(),
        1..60 => format!("{seconds} s"),
        _ => format!("{} min {} s", seconds / 60, seconds % 60),
    }
}

fn chunks(n: u32) -> String {
    plural(n as usize, "trecho", "trechos")
}

fn retry(id: DocumentId) -> Option<Action> {
    Some(Action {
        command: "retry_document",
        document_id: id,
        label: "Tentar novamente",
    })
}

fn attempts(job: &IndexJob) -> String {
    plural(job.attempts as usize, "tentativa", "tentativas")
}

impl IndexingModel {
    pub fn new(report: &IndexingReport, can_reread: bool) -> Self {
        let snapshot = &report.snapshot;
        let has_model = report.model_id.is_some();
        let (mut attention, mut done) = (Vec::new(), Vec::new());
        for job in &snapshot.jobs {
            let row = |label, kind, detail: String, meta: String, action| JobRow {
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
                        (_, false) => (
                            "Aguardando um modelo de embeddings para entrar na busca semântica."
                                .to_string(),
                            None,
                        ),
                        (None, true) => (
                            "Os embeddings ainda não foram gerados.".to_string(),
                            retry(job.document_id),
                        ),
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
                        "Nenhuma página tem texto selecionável. O reconhecimento de texto (OCR) ainda não está disponível."
                            .into(),
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
        let mut documents = vec![plural(indexed, "indexado", "indexados")];
        for (n, one, many) in [
            (snapshot.reading(), "em leitura", "em leitura"),
            (
                snapshot.count(DocumentStatus::Embedding),
                "aguardando embeddings",
                "aguardando embeddings",
            ),
            (
                snapshot.count(DocumentStatus::NeedsOcr),
                "sem texto",
                "sem texto",
            ),
            (failed, "com falha", "com falha"),
        ] {
            if n > 0 {
                documents.push(plural(n, one, many));
            }
        }
        let mut chunk_stats = vec![format!("{} no total", snapshot.chunks)];
        if has_model {
            chunk_stats.push(format!("{} com vetores", snapshot.embedded_chunks()));
        }
        if snapshot.pending_chunks() > 0 {
            chunk_stats.push(format!("{} aguardando", snapshot.pending_chunks()));
        }
        let mut stats = vec![
            Stat {
                label: "Documentos",
                value: documents.join(" · "),
            },
            Stat {
                label: "Trechos",
                value: chunk_stats.join(" · "),
            },
        ];
        if let Some(model) = &snapshot.model {
            stats.push(Stat {
                label: "Dimensões dos vetores",
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
            reindex_description: format!(
                "Os vetores de {} ({}) serão gerados de novo com o modelo ativo. A busca continua funcionando enquanto isso.",
                plural(indexed, "documento", "documentos"),
                chunks(indexed_chunks),
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

#[cfg(test)]
mod tests {
    use super::*;
    use nlmx_domain::indexing::{EmbeddedModel, IndexSnapshot};

    fn job(id: DocumentId, status: DocumentStatus) -> IndexJob {
        IndexJob {
            document_id: id,
            title: format!("Doc {id}"),
            document_type: nlmx_domain::document_type::DocumentType::Pdf,
            status,
            error: None,
            chunks: 10,
            attempts: 1,
            started_at: Some("2026-10-02T14:31:05.123Z".into()),
            finished_at: None,
            duration_ms: None,
        }
    }

    fn report(jobs: Vec<IndexJob>, model: bool, running: bool) -> IndexingReport {
        IndexingReport {
            snapshot: IndexSnapshot {
                chunks: jobs.iter().map(|j| j.chunks).sum(),
                model: model.then(|| EmbeddedModel {
                    model_id: "qwen".into(),
                    dimensions: 1024,
                    chunks: 10,
                }),
                jobs,
            },
            model_id: model.then(|| "qwen".into()),
            running,
        }
    }

    #[test]
    fn formats_durations() {
        assert_eq!(duration(400), "menos de 1 s");
        assert_eq!(duration(16_200), "16 s");
        assert_eq!(duration(125_000), "2 min 5 s");
    }

    #[test]
    fn groups_documents_by_what_they_need() {
        let mut failed = job(2, DocumentStatus::Failed);
        failed.error = Some("PDF inválido".into());
        failed.attempts = 2;
        let mut indexed = job(4, DocumentStatus::Indexed);
        indexed.finished_at = Some("2026-10-03T10:00:00.000Z".into());
        indexed.duration_ms = Some(16_000);
        let model = IndexingModel::new(
            &report(
                vec![
                    job(1, DocumentStatus::Chunking),
                    failed,
                    job(3, DocumentStatus::NeedsOcr),
                    indexed,
                    job(5, DocumentStatus::Embedding),
                ],
                true,
                false,
            ),
            true,
        );
        assert!(model.active, "a document is being read");
        assert_eq!(model.attention.len(), 3);
        assert_eq!(model.attention[0].detail, "PDF inválido");
        assert_eq!(model.attention[0].meta, "2 tentativas");
        assert_eq!(
            model.attention[0].action.as_ref().map(|a| a.document_id),
            Some(2)
        );
        assert!(model.attention[1].action.is_none(), "no OCR yet");
        assert!(model.attention[2].action.is_some(), "embed again");
        assert_eq!(model.recent[0].meta, "03/10/2026 · 16 s · 10 trechos");
        assert!(model.embed_pending && model.retry_failed && model.reindex);
        assert!(
            model
                .reindex_description
                .contains("1 documento (10 trechos)")
        );
    }

    #[test]
    fn every_row_shows_the_icon_and_format_of_its_source() {
        use nlmx_domain::document_type::DocumentType;
        let mut csv = job(1, DocumentStatus::Failed);
        csv.document_type = DocumentType::Csv;
        csv.error = Some("inválido".into());
        let mut epub = job(2, DocumentStatus::Indexed);
        epub.document_type = DocumentType::Epub;
        epub.finished_at = Some("2026-10-03T10:00:00.000Z".into());
        let model = IndexingModel::new(
            &report(
                vec![csv, epub, job(3, DocumentStatus::Embedding)],
                true,
                false,
            ),
            true,
        );
        assert_eq!(
            (model.attention[0].icon, model.attention[0].format),
            ("format-csv", "CSV")
        );
        assert_eq!(
            (model.recent[0].icon, model.recent[0].format),
            ("format-epub", "EPUB")
        );
        assert_eq!(model.attention[1].format, "PDF");
    }

    #[test]
    fn without_a_model_documents_wait_and_nothing_can_be_reindexed() {
        let model = IndexingModel::new(
            &report(vec![job(1, DocumentStatus::Embedding)], false, false),
            false,
        );
        assert!(!model.embed_pending && !model.reindex && !model.active);
        assert!(model.attention[0].action.is_none());
        assert!(model.attention[0].detail.contains("Aguardando um modelo"));
    }

    #[test]
    fn while_running_pending_documents_are_not_listed_as_needing_attention() {
        let model = IndexingModel::new(
            &report(vec![job(1, DocumentStatus::Embedding)], true, true),
            true,
        );
        assert!(model.active);
        assert!(model.attention.is_empty() && model.recent.is_empty());
    }
}
