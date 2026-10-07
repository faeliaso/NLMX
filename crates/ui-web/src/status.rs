//! Status bar fragment and the shared view of the language model status.

use askama::Template;
use axum::{
    extract::State,
    response::{Html, IntoResponse, Response},
};
use http::StatusCode;
use nlmx_application::ports::{StorageError, StorageInfo};
use nlmx_domain::generation::{LanguageModelStatus, UnavailableKind};
use nlmx_i18n::{t, t_args};

use crate::AppState;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Display data for a [`LanguageModelStatus`]: design-system badge kind, label and detail.
pub struct LanguageModelView {
    pub kind: &'static str,
    pub label: String,
    pub detail: String,
}

/// Localized `(kind, label, detail)` of a status; `None` detail means "use the reason".
fn localized(status: &LanguageModelStatus) -> (&'static str, String, Option<String>) {
    match status {
        LanguageModelStatus::Available => (
            "success",
            t("status-lm-available-label"),
            Some(t("status-lm-available-detail")),
        ),
        LanguageModelStatus::LicenseRequired => (
            "warning",
            t("status-lm-license-label"),
            Some(t("status-lm-license-detail")),
        ),
        LanguageModelStatus::NotInstalled => (
            "danger",
            t("status-lm-not-installed-label"),
            Some(t("status-lm-not-installed-detail")),
        ),
        LanguageModelStatus::Incompatible { .. } => {
            ("danger", t("status-lm-incompatible-label"), None)
        }
        LanguageModelStatus::Unavailable { kind, .. } => match kind {
            UnavailableKind::AppleIntelligenceDisabled => (
                "warning",
                t("status-lm-ai-disabled-label"),
                Some(t("status-lm-ai-disabled-detail")),
            ),
            UnavailableKind::DeviceNotEligible => (
                "danger",
                t("status-lm-device-label"),
                Some(t("status-lm-device-detail")),
            ),
            UnavailableKind::ModelNotReady => (
                "warning",
                t("status-lm-preparing-label"),
                Some(t("status-lm-preparing-detail")),
            ),
            UnavailableKind::Other => ("danger", t("status-unavailable"), None),
        },
    }
}

impl From<&LanguageModelStatus> for LanguageModelView {
    fn from(status: &LanguageModelStatus) -> Self {
        let (kind, label, detail) = localized(status);
        let detail = detail.unwrap_or_else(|| match status {
            // Technical reasons come from the runtime itself and are shown as they are.
            LanguageModelStatus::Incompatible { reason }
            | LanguageModelStatus::Unavailable { reason, .. } => reason.clone(),
            _ => String::new(),
        });
        Self {
            kind,
            label,
            detail,
        }
    }
}

/// Display data for the database diagnostics.
pub struct StorageView {
    pub ok: bool,
    pub path: String,
    pub schema: String,
    pub sqlite_version: String,
    pub vector_version: String,
    pub error: String,
}

impl From<&Result<StorageInfo, StorageError>> for StorageView {
    fn from(result: &Result<StorageInfo, StorageError>) -> Self {
        match result {
            Ok(info) => Self {
                ok: true,
                path: info.path.clone(),
                schema: t_args(
                    "status-schema-version",
                    &[
                        ("current", info.schema_version.to_string().as_str().into()),
                        (
                            "latest",
                            info.latest_schema_version.to_string().as_str().into(),
                        ),
                    ],
                ),
                sqlite_version: info.sqlite_version.clone(),
                vector_version: info.vector_extension_version.clone(),
                error: String::new(),
            },
            Err(err) => Self {
                ok: false,
                path: String::new(),
                schema: String::new(),
                sqlite_version: String::new(),
                vector_version: String::new(),
                error: err.message.clone(),
            },
        }
    }
}

#[derive(Template)]
#[template(path = "components/status.html")]
struct StatusFragment {
    lm: LanguageModelView,
    storage: StorageView,
    version: &'static str,
}

pub async fn fragment(State(state): State<AppState>) -> Response {
    let status = state.system_status.execute().await;
    let fragment = StatusFragment {
        lm: LanguageModelView::from(&status.language_model),
        storage: StorageView::from(&status.storage),
        version: VERSION,
    };
    match fragment.render() {
        Ok(html) => Html(html).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}
