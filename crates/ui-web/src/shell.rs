//! The application shell: sections, sidebar navigation and full-page vs. HTMX-partial rendering.

use askama::Template;
use axum::response::{Html, IntoResponse, Response};
use http::{HeaderMap, StatusCode};

use crate::error::{UiError, fallback_error_html};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Chat,
    Documents,
    Indexing,
    Models,
    Settings,
}

impl Section {
    /// Sidebar order; ⌘1…⌘5 follow it.
    pub const ALL: [Section; 5] = [
        Self::Chat,
        Self::Documents,
        Self::Indexing,
        Self::Models,
        Self::Settings,
    ];

    pub fn path(self) -> &'static str {
        match self {
            Self::Chat => "/chat",
            Self::Documents => "/documents",
            Self::Indexing => "/indexing",
            Self::Models => "/models",
            Self::Settings => "/settings",
        }
    }

    /// The section's name in the active language.
    pub fn label(self) -> String {
        nlmx_i18n::t(match self {
            Self::Chat => "nav-chat",
            Self::Documents => "nav-documents",
            Self::Indexing => "nav-indexing",
            Self::Models => "nav-models",
            Self::Settings => "nav-settings",
        })
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Documents => "document",
            Self::Indexing => "layers",
            Self::Models => "cube",
            Self::Settings => "settings",
        }
    }
}

pub struct NavItem {
    pub path: &'static str,
    pub label: String,
    pub icon: &'static str,
    pub current: bool,
}

pub struct Nav {
    pub items: Vec<NavItem>,
    /// Rendered as an HTMX out-of-band swap so the active item follows navigation.
    pub oob: bool,
}

impl Nav {
    fn new(current: Option<Section>, oob: bool) -> Self {
        let items = Section::ALL
            .iter()
            .map(|&section| NavItem {
                path: section.path(),
                label: section.label(),
                icon: section.icon(),
                current: Some(section) == current,
            })
            .collect();
        Self { items, oob }
    }
}

#[derive(Template)]
#[template(path = "pages/shell.html")]
struct ShellPage<'a> {
    title: &'a str,
    content: &'a str,
    nav: Nav,
}

#[derive(Template)]
#[template(path = "pages/partial.html")]
struct PartialPage<'a> {
    content: &'a str,
    nav: Nav,
}

#[derive(Template)]
#[template(path = "pages/sections/error.html")]
struct ErrorView<'a> {
    title: &'a str,
    message: &'a str,
}

/// HTMX navigation gets the section content plus an out-of-band nav update; everything else
/// (first load, reload, history restore) gets the full shell.
fn is_partial(headers: &HeaderMap) -> bool {
    headers.contains_key("hx-request") && !headers.contains_key("hx-history-restore-request")
}

/// Renders a section (or its error state) inside the shell.
pub fn page(
    headers: &HeaderMap,
    section: Option<Section>,
    content: Result<String, UiError>,
) -> Response {
    let (status, title, content) = match content {
        Ok(html) => (
            StatusCode::OK,
            section.map_or_else(|| "NLMX".to_string(), Section::label),
            html,
        ),
        Err(err) => {
            let html = ErrorView {
                title: &err.title,
                message: &err.message,
            }
            .render()
            .unwrap_or_else(|_| fallback_error_html());
            (err.status, err.title, html)
        }
    };

    let rendered = if is_partial(headers) {
        PartialPage {
            content: &content,
            nav: Nav::new(section, true),
        }
        .render()
    } else {
        ShellPage {
            title: &title,
            content: &content,
            nav: Nav::new(section, false),
        }
        .render()
    };

    match rendered {
        Ok(html) => (status, Html(html)).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(fallback_error_html()),
        )
            .into_response(),
    }
}
