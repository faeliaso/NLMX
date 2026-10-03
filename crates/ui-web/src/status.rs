//! Status bar fragment and the shared view of the language model status.

use askama::Template;
use axum::{
    extract::State,
    response::{Html, IntoResponse, Response},
};
use http::StatusCode;
use nlmx_application::ports::{StorageError, StorageInfo};
use nlmx_domain::generation::{LanguageModelStatus, UnavailableKind};

use crate::AppState;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Display data for a [`LanguageModelStatus`]: design-system badge kind, label and detail.
pub struct LanguageModelView {
    pub kind: &'static str,
    pub label: &'static str,
    pub detail: String,
}

impl From<&LanguageModelStatus> for LanguageModelView {
    fn from(status: &LanguageModelStatus) -> Self {
        match status {
            LanguageModelStatus::Available => Self {
                kind: "success",
                label: "Disponível",
                detail: "O modelo on-device do Apple Intelligence está pronto para gerar respostas.".into(),
            },
            LanguageModelStatus::LicenseRequired => Self {
                kind: "warning",
                label: "Licença pendente",
                detail: "Aceite os termos de uso uma vez neste Mac executando no Terminal: sudo fm license".into(),
            },
            LanguageModelStatus::Incompatible { reason } => Self {
                kind: "danger",
                label: "Incompatível",
                detail: reason.clone(),
            },
            LanguageModelStatus::Unavailable { kind, reason } => match kind {
                UnavailableKind::AppleIntelligenceDisabled => Self {
                    kind: "warning",
                    label: "Apple Intelligence desativado",
                    detail: "Ative o Apple Intelligence em Ajustes do Sistema para gerar respostas.".into(),
                },
                UnavailableKind::DeviceNotEligible => Self {
                    kind: "danger",
                    label: "Mac não compatível",
                    detail: "Este Mac não é compatível com o Apple Intelligence.".into(),
                },
                UnavailableKind::ModelNotReady => Self {
                    kind: "warning",
                    label: "Preparando o modelo",
                    detail: "O macOS ainda está baixando ou preparando o modelo. Tente novamente em alguns minutos.".into(),
                },
                UnavailableKind::Other => Self {
                    kind: "danger",
                    label: "Indisponível",
                    detail: reason.clone(),
                },
            },
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
                schema: format!("{} de {}", info.schema_version, info.latest_schema_version),
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
