//! "Ative a IA local": the dialog that guides the one-time Apple Foundation Models license.
//!
//! The dialog only ever *reads* the real state of `fm` (`GetSystemStatus`); the user runs
//! `sudo fm license` in Terminal themselves. Nothing here executes it, asks for a password or
//! remembers that the dialog was seen — whether it shows depends on the current status alone.

use askama::Template;
use axum::{
    extract::State,
    response::{Html, IntoResponse, Response},
};
use http::StatusCode;
use nlmx_application::use_cases::describe_model_status;
use nlmx_domain::generation::{LanguageModelStatus, UnavailableKind};

use crate::AppState;

/// The command the user is asked to run. Copied verbatim, never executed by the app.
pub const LICENSE_COMMAND: &str = "sudo fm license";

const NOT_INSTALLED_HINT: &str = "Verifique se sua versão do macOS e os componentes necessários para o Apple Foundation Models estão disponíveis.";

/// What the dialog shows for a status. `None` when the model is ready.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// The license is pending: steps, command and password help.
    License,
    /// `fm` is missing or this Mac can't run it: no `sudo fm license` here.
    Unavailable { detail: String, hint: &'static str },
    /// The check itself failed: friendly message and "Tentar novamente".
    Problem,
    /// Authorized.
    Ready,
}

impl Stage {
    pub fn of(status: &LanguageModelStatus) -> Self {
        match status {
            LanguageModelStatus::Available => Self::Ready,
            LanguageModelStatus::LicenseRequired => Self::License,
            LanguageModelStatus::NotInstalled => Self::Unavailable {
                detail: "O NLMX não encontrou o fm neste Mac.".into(),
                hint: NOT_INSTALLED_HINT,
            },
            LanguageModelStatus::Incompatible { reason } => Self::Unavailable {
                detail: reason.clone(),
                hint: NOT_INSTALLED_HINT,
            },
            LanguageModelStatus::Unavailable {
                kind: UnavailableKind::Other,
                ..
            } => Self::Problem,
            LanguageModelStatus::Unavailable { .. } => Self::Unavailable {
                detail: describe_model_status(status),
                hint: "",
            },
        }
    }
}

#[derive(Template)]
#[template(path = "components/fm_setup_body.html")]
struct Body {
    stage: Stage,
    command: &'static str,
    /// The user asked to check again and the state did not change.
    rechecked: bool,
}

#[derive(Template)]
#[template(path = "components/fm_setup.html")]
struct Dialog {
    body: Body,
}

fn render(template: impl Template) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

/// Loaded once per full page load (app start): empty when authorized, the dialog otherwise.
pub async fn fragment(State(state): State<AppState>) -> Response {
    let status = state.system_status.language_model().await;
    match Stage::of(&status) {
        Stage::Ready => Html(String::new()).into_response(),
        stage => render(Dialog {
            body: Body {
                stage,
                command: LICENSE_COMMAND,
                rechecked: false,
            },
        }),
    }
}

/// "Verificar novamente": asks `fm` again (never the cache) and returns the dialog's new content.
pub async fn recheck(State(state): State<AppState>) -> Response {
    let status = state.system_status.recheck_language_model().await;
    render(Body {
        stage: Stage::of(&status),
        command: LICENSE_COMMAND,
        rechecked: true,
    })
}
