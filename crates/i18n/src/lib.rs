//! Interface localization (ADR 0022).
//!
//! The UI language is resolved as: explicit user preference → macOS language (when supported)
//! → English. It is independent of the language the model answers in (`ResponseLanguage` in the
//! domain). The app is single-user, so the active locale is process-wide ([`current`]); code
//! that needs a specific locale (tests, formatting) uses the `*_in` functions.
//!
//! Catalogs are Project Fluent files embedded at build time (`locales/<tag>.ftl`). Message ids
//! are semantic (`chat-send`, `sources-empty`); a missing id in a locale falls back to English
//! and never reaches the user as an id.

mod catalog;
mod format;
mod locale;

pub use catalog::{Arg, catalog_source, message_ids, namespaces, tr, tr_args};
pub use format::{format_bytes, format_integer, format_number, format_percent};
pub use locale::{Locale, current, resolve, set_current};

/// Translates `key` in the active locale.
pub fn t(key: &str) -> String {
    tr(current(), key)
}

/// Translates `key` in the active locale with arguments (`{ $name }` in the catalog).
pub fn t_args(key: &str, args: &[(&str, Arg)]) -> String {
    tr_args(current(), key, args)
}

/// Translates a message that only takes a count (`{ $count }`, plural selectors).
pub fn t_count(key: &str, count: i64) -> String {
    t_args(key, &[("count", Arg::Num(count as f64))])
}
