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
        .body(nlmx_ui_web::FALLBACK_ERROR_HTML.as_bytes().to_vec())
        .expect("valid error response")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nlmx_application::use_cases::GetSystemStatus;
    use nlmx_testing::{FakeLlmProvider, FakeStorage};
    use nlmx_ui_web::AppState;

    use super::*;

    fn router() -> Router {
        nlmx_ui_web::router(AppState {
            system_status: Arc::new(GetSystemStatus::new(
                Arc::new(FakeLlmProvider::available()),
                Arc::new(FakeStorage::healthy()),
                Ok("chromium/7881".into()),
                Arc::new(nlmx_testing::FakeRuntime::llama()),
            )),
            ingestion: Err("não usado neste teste".into()),
            chat: Err("não usado neste teste".into()),
            viewer: Err("não usado neste teste".into()),
            remover: Err("remoção indisponível neste teste".into()),
            diagnostics: None,
            models: None,
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

    #[test]
    fn start_url_defaults_to_root() {
        if std::env::var("NLMX_START_PATH").is_err() {
            assert_eq!(start_url().as_str(), "nlmx://localhost/");
        }
    }
}
