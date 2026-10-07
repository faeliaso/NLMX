//! Design-system gallery (debug builds only): every token and component in every state.

use askama::Template;
use axum::response::{Html, IntoResponse, Response};
use http::{StatusCode, Uri};

const TOKENS: &str = include_str!("../../../apps/desktop/ui/styles/tokens.css");

#[derive(Template)]
#[template(path = "pages/design-system.html")]
struct GalleryPage {
    theme: Option<&'static str>,
    colors: Vec<&'static str>,
}

/// Color token names, read from tokens.css so the gallery never drifts from the source.
fn color_tokens() -> Vec<&'static str> {
    TOKENS
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("--color-")?
                .split_once(':')
                .map(|(name, _)| name)
        })
        .filter(|name| *name != "*")
        .collect()
}

/// `?theme=light|dark` forces a theme (used for screenshots and review).
fn forced_theme(uri: &Uri) -> Option<&'static str> {
    let query = uri.query()?;
    query.split('&').find_map(|pair| match pair {
        "theme=light" => Some("light"),
        "theme=dark" => Some("dark"),
        _ => None,
    })
}

pub async fn page(uri: Uri) -> Response {
    let gallery = GalleryPage {
        theme: forced_theme(&uri),
        colors: color_tokens(),
    };
    match gallery.render() {
        Ok(html) => Html(html).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}
