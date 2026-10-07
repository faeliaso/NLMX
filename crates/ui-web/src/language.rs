//! Interface language (ADR 0022): resolution at startup, the picker in Configurações and the
//! switch at runtime. Independent of the language the model answers in.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use axum::{
    extract::{Form, State},
    response::{IntoResponse, Response},
};
use http::{HeaderValue, StatusCode};
use nlmx_application::ports::SettingsRepository;
use nlmx_i18n::{Locale, resolve, set_current};
use serde::Deserialize;

use crate::AppState;

/// `app_settings` key holding the explicit choice (`"pt-BR"`, `"en"` or `"es"`); absent until the
/// user picks a language.
pub const SETTING_KEY: &str = "ui.language";

/// Where the preference lives and what the system language is.
#[derive(Clone)]
pub struct LanguageSettings {
    store: Option<Arc<dyn SettingsRepository>>,
    system: Vec<String>,
    loaded: Arc<AtomicBool>,
}

impl LanguageSettings {
    /// `system` are the macOS preferred languages, most preferred first.
    pub fn new(store: Option<Arc<dyn SettingsRepository>>, system: Vec<String>) -> Self {
        Self {
            store,
            system,
            loaded: Arc::default(),
        }
    }

    /// Settings that never read nor save (tests, no database): English unless `system` says so.
    pub fn ephemeral(system: Vec<String>) -> Self {
        Self::new(None, system)
    }

    async fn saved(&self) -> Option<Locale> {
        let raw = self.store.as_ref()?.get(SETTING_KEY).await.ok()??;
        serde_json::from_str::<String>(&raw)
            .ok()
            .and_then(|tag| Locale::from_tag(&tag))
    }

    /// Resolves the language once (saved choice → macOS → English) before the first request.
    pub async fn ensure_loaded(&self) {
        if self.loaded.load(Ordering::Acquire) {
            return;
        }
        set_current(resolve(self.saved().await, &self.system));
        self.loaded.store(true, Ordering::Release);
    }
}

pub async fn load_before_request(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    state.language.ensure_loaded().await;
    next.run(request).await
}

/// The picker's options: tag, native name and whether it is the active one.
pub fn options() -> Vec<(&'static str, &'static str, bool)> {
    let current = nlmx_i18n::current();
    Locale::ALL
        .iter()
        .map(|l| (l.tag(), l.native_name(), *l == current))
        .collect()
}

#[derive(Deserialize)]
pub struct LanguageForm {
    language: String,
}

/// `POST /settings/language`: saves the choice and switches at once. The page reloads its
/// content on the `language-changed` event (no window reload).
pub async fn set(State(state): State<AppState>, Form(form): Form<LanguageForm>) -> Response {
    let Some(locale) = Locale::from_tag(&form.language) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if let Some(store) = &state.language.store {
        let json = serde_json::to_string(locale.tag()).unwrap_or_default();
        if store.set(SETTING_KEY, &json).await.is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    state.language.loaded.store(true, Ordering::Release);
    set_current(locale);
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert("HX-Trigger", HeaderValue::from_static("language-changed"));
    response
}
