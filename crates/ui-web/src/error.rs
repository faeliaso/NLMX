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
            title: "Página não encontrada".into(),
            message: "O conteúdo solicitado não existe nesta versão do aplicativo.".into(),
        }
    }

    pub fn internal(detail: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            title: "Erro interno".into(),
            message: format!("Não foi possível exibir esta tela ({detail})."),
        }
    }
}

impl From<askama::Error> for UiError {
    fn from(err: askama::Error) -> Self {
        Self::internal(err)
    }
}

/// Last-resort body used when rendering itself fails (e.g. a handler panicked).
pub const FALLBACK_ERROR_HTML: &str = r#"<div class="page"><div class="page-body"><div class="alert alert-danger" role="alert"><div class="alert-content"><p class="alert-title">Erro interno</p><p>Ocorreu uma falha inesperada. Tente novamente.</p></div></div></div></div>"#;
