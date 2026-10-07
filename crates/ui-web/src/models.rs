//! Modelos: Apple FM status, the bundled runtime and the embedding model catalog.

use askama::Template;
use axum::{
    extract::State,
    response::{Html, IntoResponse, Response},
};
use http::{HeaderMap, StatusCode};
use nlmx_application::ports::ModelProvider;
use nlmx_domain::models::{DownloadPlan, ModelState, RuntimeInfo};
use nlmx_i18n::{Arg, current, format_bytes, t, t_args};

use crate::{
    AppState,
    error::UiError,
    shell::{self, Section},
    status::LanguageModelView,
};

pub fn bytes(n: u64) -> String {
    format_bytes(current(), n)
}

fn s(v: &str) -> Arg {
    Arg::Str(v.to_string())
}

pub struct PlanView {
    pub size: String,
    pub fits: bool,
    pub resume: String,
    pub replaces: String,
    /// "required (available free)".
    pub space: String,
    /// The confirm button: "Download 640 MB".
    pub download_label: String,
}

impl From<&DownloadPlan> for PlanView {
    fn from(p: &DownloadPlan) -> Self {
        Self {
            size: bytes(p.model.size),
            fits: p.fits_on_disk(),
            resume: if p.resume_from > 0 {
                t_args("models-plan-resume", &[("size", s(&bytes(p.resume_from)))])
            } else {
                String::new()
            },
            space: t_args(
                "models-plan-space-value",
                &[
                    ("required", s(&bytes(p.required_bytes))),
                    ("available", s(&bytes(p.available_bytes))),
                ],
            ),
            download_label: t_args(
                "models-plan-download-size",
                &[("size", s(&bytes(p.model.size)))],
            ),
            replaces: p
                .replaces
                .clone()
                .map(|v| v.chars().take(8).collect())
                .unwrap_or_default(),
        }
    }
}

pub struct ModelRow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub license_id: String,
    pub license_url: String,
    pub recommended: bool,
    pub state_label: String,
    pub state_kind: &'static str,
    pub active: bool,
    pub installed: bool,
    pub corrupted: bool,
    pub plan: Option<PlanView>,
    pub plan_is_update: bool,
    /// "640 MB · 1024 dimensions".
    pub size_dimensions: String,
    pub progress_label: String,
    pub more_actions_label: String,
    pub remove_title: String,
    pub plan_title: String,
}

pub struct Catalog {
    pub rows: Vec<ModelRow>,
    pub disk: String,
}

async fn catalog(models: &dyn ModelProvider) -> Catalog {
    let active = models.active().await.ok().flatten().map(|m| m.id);
    let mut rows = Vec::new();
    for m in models.catalog() {
        let state = models
            .status(&m.id)
            .await
            .unwrap_or(ModelState::NotInstalled);
        let is_active = active.as_deref() == Some(m.id.as_str());
        let (state_label, state_kind, installed, corrupted) = match &state {
            ModelState::NotInstalled => (t("models-state-not-installed"), "neutral", false, false),
            ModelState::PartiallyDownloaded { bytes: b, total } => (
                t_args(
                    "models-state-partial",
                    &[("done", s(&bytes(*b))), ("total", s(&bytes(*total)))],
                ),
                "warning",
                false,
                false,
            ),
            ModelState::Installed { .. } if is_active => {
                (t("models-state-active"), "success", true, false)
            }
            ModelState::Installed { .. } => (t("models-state-installed"), "info", true, false),
            ModelState::UpdateAvailable { .. } => (t("models-state-update"), "accent", true, false),
            ModelState::Corrupted { reason } => (
                t_args("models-state-corrupted", &[("reason", s(reason))]),
                "danger",
                false,
                true,
            ),
        };
        let (plan, plan_is_update) = match &state {
            ModelState::UpdateAvailable { .. } => (models.plan_update(&m.id).await.ok(), true),
            ModelState::Installed { .. } => (None, false),
            _ => (models.plan_download(&m.id).await.ok(), false),
        };
        rows.push(ModelRow {
            id: m.id.clone(),
            name: m.display_name.clone(),
            description: m.description.clone(),
            license_id: m.license.id.clone(),
            license_url: m.license.url.clone(),
            recommended: m.recommended,
            state_label,
            state_kind,
            active: is_active,
            installed,
            corrupted,
            plan: plan.as_ref().map(PlanView::from),
            plan_is_update,
            size_dimensions: t_args(
                "models-size-dimensions",
                &[
                    ("size", s(&bytes(m.size))),
                    ("dimensions", s(&m.dimensions.to_string())),
                ],
            ),
            progress_label: t_args("models-progress-aria", &[("name", s(&m.display_name))]),
            more_actions_label: t_args("models-more-actions", &[("name", s(&m.display_name))]),
            remove_title: t_args("models-remove-title", &[("name", s(&m.display_name))]),
            plan_title: if plan_is_update {
                t_args("models-plan-update-title", &[("name", s(&m.display_name))])
            } else {
                t_args(
                    "models-plan-download-title",
                    &[("name", s(&m.display_name))],
                )
            },
        });
    }
    let disk = models
        .disk()
        .await
        .map(|d| {
            t_args(
                "models-disk",
                &[
                    ("models", s(&bytes(d.models_bytes))),
                    ("available", s(&bytes(d.available_bytes))),
                ],
            )
        })
        .unwrap_or_default();
    Catalog { rows, disk }
}

#[derive(Template)]
#[template(path = "pages/sections/models.html")]
struct ModelsView {
    lm: LanguageModelView,
    runtime: Result<RuntimeInfo, String>,
    catalog: Option<Catalog>,
}

#[derive(Template)]
#[template(path = "components/model_list.html")]
struct ModelListFragment {
    catalog: Option<Catalog>,
}

pub async fn page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let status = state.system_status.execute().await;
    let catalog = match &state.models {
        Some(models) => Some(catalog(models.as_ref()).await),
        None => None,
    };
    let view = ModelsView {
        lm: LanguageModelView::from(&status.language_model),
        runtime: status.runtime,
        catalog,
    };
    shell::page(
        &headers,
        Some(Section::Models),
        view.render().map_err(UiError::from),
    )
}

/// Re-rendered after a model command (`models-changed` event).
pub async fn fragment(State(state): State<AppState>) -> Response {
    let catalog = match &state.models {
        Some(models) => Some(catalog(models.as_ref()).await),
        None => None,
    };
    match (ModelListFragment { catalog }).render() {
        Ok(html) => Html(html).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}
