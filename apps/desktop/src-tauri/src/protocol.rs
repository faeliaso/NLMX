//! Bridges the Tauri custom URI scheme to the in-process axum router.

use axum::{Router, body::Body};
use http::{Request, Response, StatusCode, header::CONTENT_TYPE};
use tauri::{Manager, Runtime, UriSchemeContext, UriSchemeResponder, Url};
use tower::ServiceExt;

use crate::wiring::UiRouter;

pub const SCHEME: &str = "nlmx";

/// In debug builds `NLMX_START_PATH` (e.g. `/design-system?theme=dark`) overrides the start page.
pub fn start_url() -> Url {
    let path = if cfg!(debug_assertions) {
        std::env::var("NLMX_START_PATH").unwrap_or_else(|_| "/".into())
    } else {
        "/".into()
    };
    format!("nlmx://localhost{path}")
        .parse()
        .unwrap_or_else(|_| "nlmx://localhost/".parse().expect("valid start URL"))
}

pub fn handle<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    // The router is registered in `setup`, before the window that issues requests exists.
    let Some(router) = ctx
        .app_handle()
        .try_state::<UiRouter>()
        .map(|state| state.0.clone())
    else {
        responder.respond(internal_error());
        return;
    };

    tauri::async_runtime::spawn(async move {
        let path = request.uri().path().to_string();
        // Run the handler in its own task so a panic becomes a 500 instead of a hung request.
        let response = match tauri::async_runtime::spawn(dispatch(router, request)).await {
            Ok(response) => response,
            Err(err) => {
                tracing::error!(%path, "UI handler failed: {err}");
                internal_error()
            }
        };
        responder.respond(response);
    });
}

async fn dispatch(router: Router, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let (parts, body) = request.into_parts();
    let response = match router
        .oneshot(Request::from_parts(parts, Body::from(body)))
        .await
    {
        Ok(response) => response,
        Err(never) => match never {},
    };

    let (parts, body) = response.into_parts();
    match axum::body::to_bytes(body, usize::MAX).await {
        Ok(bytes) => Response::from_parts(parts, bytes.to_vec()),
        Err(err) => {
            tracing::error!("failed to read UI response body: {err}");
            internal_error()
        }
    }
}

fn internal_error() -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .body(nlmx_ui_web::fallback_error_html().into_bytes())
        .expect("valid error response")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nlmx_application::use_cases::GetSystemStatus;
    use nlmx_domain::generation::LanguageModelStatus;
    use nlmx_testing::{FakeLlmProvider, FakeStorage};
    use nlmx_ui_web::AppState;

    use super::*;

    fn router() -> Router {
        router_with(Arc::new(FakeLlmProvider::available()))
    }

    fn router_with(llm: Arc<FakeLlmProvider>) -> Router {
        router_diagnosed(llm, None)
    }

    fn router_diagnosed(
        llm: Arc<FakeLlmProvider>,
        diagnostics: Option<Arc<dyn nlmx_application::ports::Diagnostics>>,
    ) -> Router {
        nlmx_ui_web::router(AppState {
            system_status: Arc::new(GetSystemStatus::new(
                llm,
                Arc::new(FakeStorage::healthy()),
                Ok("chromium/7881".into()),
                Arc::new(nlmx_testing::FakeRuntime::llama()),
            )),
            ingestion: Err("não usado neste teste".into()),
            chat: Err("não usado neste teste".into()),
            viewer: Err("não usado neste teste".into()),
            remover: Err("remoção indisponível neste teste".into()),
            diagnostics,
            models: None,
            indexing: Err("indexação indisponível neste teste".into()),
            notes: None,
            language: nlmx_ui_web::LanguageSettings::ephemeral(vec!["pt-BR".into()]),
        })
    }

    #[tokio::test]
    async fn routes_absolute_custom_scheme_uris() {
        let request = Request::get("nlmx://localhost/fragments/status")
            .body(Vec::new())
            .unwrap();
        let response = dispatch(router(), request).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(String::from_utf8_lossy(response.body()).contains("Disponível"));
    }

    /// A session whose latest answer (turn 1) used 101 prompt tokens and produced 19.
    struct CountedSession;

    impl nlmx_application::ports::Diagnostics for CountedSession {
        fn snapshot(&self) -> nlmx_application::ports::DiagnosticsSnapshot {
            nlmx_application::ports::DiagnosticsSnapshot {
                last_generation: Some(nlmx_domain::generation::LastGeneration {
                    turn: 1,
                    usage: nlmx_domain::generation::GenerationUsage {
                        prompt_tokens: 101,
                        completion_tokens: 19,
                    },
                }),
                ..Default::default()
            }
        }

        fn sample(&self) -> nlmx_application::ports::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
    }

    const USAGE: &str = "/fragments/token-usage";

    #[tokio::test]
    async fn usage_is_a_dash_before_the_first_answer() {
        let html = body_of(&router(), "GET", USAGE).await;
        assert!(html.contains("data-usage=\"idle\""), "{html}");
        assert!(html.contains(">—<"), "{html}");
        assert!(!html.contains("8.192"), "{html}");
        assert!(html.contains("Contado pelo modelo neste Mac"), "{html}");
    }

    #[tokio::test]
    async fn usage_fragment_is_never_cached() {
        let request = Request::get("nlmx://localhost/fragments/token-usage")
            .body(Vec::new())
            .unwrap();
        let response = dispatch(router(), request).await;
        assert_eq!(response.headers()["cache-control"], "no-store");
    }

    #[tokio::test]
    async fn usage_is_calculating_while_an_answer_is_generated() {
        let counted = router_diagnosed(
            Arc::new(FakeLlmProvider::available()),
            Some(Arc::new(CountedSession)),
        );
        let html = body_of(&counted, "GET", &format!("{USAGE}?busy=1")).await;
        assert!(html.contains("data-usage=\"calculating\""), "{html}");
        assert!(html.contains("Calculando…"), "{html}");
        assert!(!html.contains("101"), "no stale counts: {html}");
    }

    #[tokio::test]
    async fn usage_shows_the_counts_of_the_last_answer() {
        let counted = router_diagnosed(
            Arc::new(FakeLlmProvider::available()),
            Some(Arc::new(CountedSession)),
        );
        let html = body_of(&counted, "GET", USAGE).await;
        assert!(html.contains("data-usage=\"counted\""), "{html}");
        assert!(
            html.contains("system · turno 1 · ↑ 101 · ↓ 19 · 120 / 8.192 (1% usado)"),
            "{html}"
        );
        assert!(html.contains("Uso de tokens da última resposta"), "{html}");
    }

    #[tokio::test]
    async fn the_status_bar_hosts_the_usage_before_the_model_status() {
        let html = body_of(&router(), "GET", "/settings").await;
        let usage = html.find("id=\"token-usage\"").expect("usage slot");
        let model = html.find("id=\"system-status\"").expect("model slot");
        assert!(usage < model);
    }

    async fn body_of(router: &Router, method: &str, path: &str) -> String {
        let request = Request::builder()
            .method(method)
            .uri(format!("nlmx://localhost{path}"))
            .body(Vec::new())
            .unwrap();
        let response = dispatch(router.clone(), request).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        String::from_utf8_lossy(response.body()).into_owned()
    }

    const SETUP: &str = "/fragments/fm-setup";
    const RECHECK: &str = "/fragments/fm-setup/recheck";

    #[tokio::test]
    async fn fm_setup_is_absent_when_authorized() {
        let llm = Arc::new(FakeLlmProvider::available());
        let html = body_of(&router_with(llm.clone()), "GET", SETUP).await;
        assert!(html.trim().is_empty(), "{html}");
        assert_eq!(
            llm.recheck_calls(),
            0,
            "startup reads the status, no forced re-check"
        );
    }

    #[tokio::test]
    async fn fm_setup_opens_with_the_exact_command_when_not_authorized() {
        let llm = Arc::new(FakeLlmProvider::new(LanguageModelStatus::LicenseRequired));
        let html = body_of(&router_with(llm), "GET", SETUP).await;
        assert!(html.contains("Ative a IA local"));
        assert!(html.contains("O NLMX usa o Apple Foundation Models diretamente no seu Mac."));
        assert!(html.contains("autorização única"));
        assert!(html.contains("Não sabe qual senha usar?"));
        assert!(html.contains("nenhum caractere aparece na tela"));
        assert!(html.contains("não tiver privilégios administrativos"));
        // The copy button copies this template's text — exactly the command, nothing else.
        assert!(html.contains(r#"<template id="fm-license-command">sudo fm license</template>"#));
        assert!(html.contains(r#"data-copy-from="fm-license-command""#));
        // Copying is client-side only: no Tauri command, no request, nothing executed.
        assert!(!html.contains("data-command"));
        assert!(!html.contains("crie uma senha"));
    }

    #[tokio::test]
    async fn fm_setup_for_a_missing_fm_does_not_offer_the_license_command() {
        let llm = Arc::new(FakeLlmProvider::new(LanguageModelStatus::NotInstalled));
        let html = body_of(&router_with(llm), "GET", SETUP).await;
        assert!(html.contains("Apple Foundation Models indisponível"));
        assert!(html.contains("O NLMX não encontrou o fm neste Mac."));
        assert!(!html.contains("sudo fm license"));
        assert!(!html.contains("fm-license-command"));
    }

    #[tokio::test]
    async fn fm_setup_for_a_failed_check_is_friendly_and_has_no_technical_detail() {
        let llm = Arc::new(FakeLlmProvider::new(LanguageModelStatus::Unavailable {
            kind: nlmx_domain::generation::UnavailableKind::Other,
            reason: "Traceback at /Users/someone/secret/path".into(),
        }));
        let html = body_of(&router_with(llm), "GET", SETUP).await;
        assert!(html.contains("Não foi possível verificar o estado da IA local"));
        assert!(html.contains("Tentar novamente"));
        assert!(!html.contains("/Users/someone"));
        assert!(!html.contains("Traceback"));
        assert!(!html.contains("sudo fm license"));
    }

    #[tokio::test]
    async fn recheck_asks_again_keeps_the_dialog_while_pending_and_closes_when_authorized() {
        let llm = Arc::new(FakeLlmProvider::new(LanguageModelStatus::LicenseRequired));
        let router = router_with(llm.clone());

        let pending = body_of(&router, "POST", RECHECK).await;
        assert_eq!(llm.recheck_calls(), 1);
        assert!(pending.contains("A autorização ainda não foi identificada."));
        assert!(pending.contains("Execute sudo fm license no Terminal e tente novamente."));
        assert!(!pending.contains("data-fm-setup-done"));

        // The user ran `sudo fm license` in Terminal and came back.
        llm.set_status(LanguageModelStatus::Available);
        let done = body_of(&router, "POST", RECHECK).await;
        assert_eq!(llm.recheck_calls(), 2);
        assert!(done.contains("data-fm-setup-done"));
        assert!(done.contains("IA local ativada"));
        assert!(done.contains("O Apple Foundation Models está pronto para uso."));
    }

    #[tokio::test]
    async fn a_later_launch_follows_the_real_state_not_a_remembered_one() {
        let llm = Arc::new(FakeLlmProvider::available());
        let router = router_with(llm.clone());
        assert!(body_of(&router, "GET", SETUP).await.trim().is_empty());
        assert!(body_of(&router, "GET", SETUP).await.trim().is_empty());
        // Authorization is gone at a later launch: the dialog comes back.
        llm.set_status(LanguageModelStatus::LicenseRequired);
        assert!(
            body_of(&router, "GET", SETUP)
                .await
                .contains("Ative a IA local")
        );
    }

    #[test]
    fn start_url_defaults_to_root() {
        if std::env::var("NLMX_START_PATH").is_err() {
            assert_eq!(start_url().as_str(), "nlmx://localhost/");
        }
    }
}
