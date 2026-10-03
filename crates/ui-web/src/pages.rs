//! Page and fragment handlers.

use askama::Template;
use axum::response::{Html, IntoResponse, Response};
use http::StatusCode;

#[derive(Template)]
#[template(path = "pages/index.html")]
struct IndexPage;

#[derive(Template)]
#[template(path = "components/status.html")]
struct StatusFragment {
    version: &'static str,
}

pub async fn index() -> Response {
    render(IndexPage)
}

pub async fn status() -> Response {
    render(StatusFragment {
        version: env!("CARGO_PKG_VERSION"),
    })
}

fn render(template: impl Template) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}
