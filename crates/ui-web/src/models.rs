//! Modelos: Apple FM status, the bundled runtime and the embedding model catalog.

use askama::Template;
use axum::{
    extract::State,
    response::{Html, IntoResponse, Response},
};
use http::{HeaderMap, StatusCode};
use nlmx_application::ports::ModelProvider;
use nlmx_domain::models::{DownloadPlan, ModelState, RuntimeInfo};

use crate::{
    AppState,
    error::UiError,
    shell::{self, Section},
    status::LanguageModelView,
};

pub fn bytes(n: u64) -> String {
    let gb = n as f64 / 1_000_000_000.0;
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", n as f64 / 1_000_000.0)
    }
}

pub struct PlanView {
    pub size: String,
    pub required: String,
    pub available: String,
    pub fits: bool,
    pub resume: String,
    pub replaces: String,
}

impl From<&DownloadPlan> for PlanView {
    fn from(p: &DownloadPlan) -> Self {
        Self {
            size: bytes(p.model.size),
            required: bytes(p.required_bytes),
            available: bytes(p.available_bytes),
            fits: p.fits_on_disk(),
            resume: if p.resume_from > 0 {
                format!("Retoma de {} já baixados.", bytes(p.resume_from))
            } else {
                String::new()
            },
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
    pub size: String,
    pub dimensions: u32,
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
            ModelState::NotInstalled => ("Não instalado".to_string(), "neutral", false, false),
            ModelState::PartiallyDownloaded { bytes: b, total } => (
                format!("Download parcial ({} de {})", bytes(*b), bytes(*total)),
                "warning",
                false,
                false,
            ),
            ModelState::Installed { .. } if is_active => {
                ("Em uso".to_string(), "success", true, false)
            }
            ModelState::Installed { .. } => ("Instalado".to_string(), "info", true, false),
            ModelState::UpdateAvailable { .. } => {
                ("Atualização disponível".to_string(), "accent", true, false)
            }
            ModelState::Corrupted { reason } => {
                (format!("Corrompido: {reason}"), "danger", false, true)
            }
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
            size: bytes(m.size),
            dimensions: m.dimensions,
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
        });
    }
    let disk = models
        .disk()
        .await
        .map(|d| {
            format!(
                "{} em modelos · {} livres",
                bytes(d.models_bytes),
                bytes(d.available_bytes)
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
