//! UI errors, rendered as the error state of the design system.

use http::StatusCode;

#[derive(Debug)]
pub struct UiError {
    pub status: StatusCode,
    pub title: String,
    pub message: String,
}

impl UiError {
    pub fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            title: nlmx_i18n::t("errors-not-found-title"),
            message: nlmx_i18n::t("errors-not-found-message"),
        }
    }

    pub fn internal(detail: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            title: nlmx_i18n::t("errors-internal-title"),
            message: nlmx_i18n::t_args(
                "errors-internal-message",
                &[("detail", detail.to_string().into())],
            ),
        }
    }
}

impl From<askama::Error> for UiError {
    fn from(err: askama::Error) -> Self {
        Self::internal(err)
    }
}

/// Last-resort body used when rendering itself fails (e.g. a handler panicked).
pub fn fallback_error_html() -> String {
    format!(
        r#"<div class="page"><div class="page-body"><div class="alert alert-danger" role="alert"><div class="alert-content"><p class="alert-title">{}</p><p>{}</p></div></div></div></div>"#,
        crate::markdown::escape(&nlmx_i18n::t("errors-internal-title")),
        crate::markdown::escape(&nlmx_i18n::t("errors-fallback-message")),
    )
}
