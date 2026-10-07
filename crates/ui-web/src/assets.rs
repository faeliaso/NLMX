//! Static assets embedded in the binary, so the UI works fully offline.

use axum::response::IntoResponse;
use http::header::{CACHE_CONTROL, CONTENT_TYPE};

const CSS: &str = "text/css; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";

static APP_CSS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/app.css"));
static HTMX_JS: &[u8] = include_bytes!("../../../apps/desktop/ui/vendor/htmx.min.js");
static DS_JS: &[u8] = include_bytes!("../../../apps/desktop/ui/scripts/ds.js");
static APP_JS: &[u8] = include_bytes!("../../../apps/desktop/ui/scripts/app.js");
static VIEWER_JS: &[u8] = include_bytes!("../../../apps/desktop/ui/scripts/viewer.js");

fn asset(content_type: &'static str, body: &'static [u8]) -> impl IntoResponse {
    (
        [(CONTENT_TYPE, content_type), (CACHE_CONTROL, "no-cache")],
        body,
    )
}

pub async fn app_css() -> impl IntoResponse {
    asset(CSS, APP_CSS)
}

pub async fn htmx_js() -> impl IntoResponse {
    asset(JS, HTMX_JS)
}

pub async fn ds_js() -> impl IntoResponse {
    asset(JS, DS_JS)
}

pub async fn app_js() -> impl IntoResponse {
    asset(JS, APP_JS)
}

pub async fn viewer_js() -> impl IntoResponse {
    asset(JS, VIEWER_JS)
}
