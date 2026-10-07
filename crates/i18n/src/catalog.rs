use std::collections::BTreeSet;
use std::sync::OnceLock;

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource, FluentValue};
use unic_langid::LanguageIdentifier;

use crate::Locale;

const NAMESPACES: [&str; 14] = [
    "common",
    "nav",
    "status",
    "fm",
    "documents",
    "sources",
    "chat",
    "viewer",
    "models",
    "indexing",
    "settings",
    "diagnostics",
    "errors",
    "js",
];

macro_rules! catalog_files {
    ($tag:literal) => {
        [
            include_str!(concat!("../locales/", $tag, "/common.ftl")),
            include_str!(concat!("../locales/", $tag, "/nav.ftl")),
            include_str!(concat!("../locales/", $tag, "/status.ftl")),
            include_str!(concat!("../locales/", $tag, "/fm.ftl")),
            include_str!(concat!("../locales/", $tag, "/documents.ftl")),
            include_str!(concat!("../locales/", $tag, "/sources.ftl")),
            include_str!(concat!("../locales/", $tag, "/chat.ftl")),
            include_str!(concat!("../locales/", $tag, "/viewer.ftl")),
            include_str!(concat!("../locales/", $tag, "/models.ftl")),
            include_str!(concat!("../locales/", $tag, "/indexing.ftl")),
            include_str!(concat!("../locales/", $tag, "/settings.ftl")),
            include_str!(concat!("../locales/", $tag, "/diagnostics.ftl")),
            include_str!(concat!("../locales/", $tag, "/errors.ftl")),
            include_str!(concat!("../locales/", $tag, "/js.ftl")),
        ]
    };
}

/// The catalog of a locale: its namespace files (`locales/<tag>/<namespace>.ftl`) joined.
/// Message ids start with their namespace (`chat-send` lives in `chat.ftl`).
pub fn catalog_source(locale: Locale) -> &'static str {
    static SOURCES: OnceLock<[String; 3]> = OnceLock::new();
    let all = SOURCES.get_or_init(|| {
        [
            catalog_files!("pt-BR").join("\n"),
            catalog_files!("en").join("\n"),
            catalog_files!("es").join("\n"),
        ]
    });
    &all[Locale::ALL.iter().position(|l| *l == locale).unwrap_or(1)]
}

/// Namespaces (file names, without extension) of the catalogs.
pub fn namespaces() -> &'static [&'static str] {
    &NAMESPACES
}

/// A message argument.
#[derive(Debug, Clone)]
pub enum Arg {
    Str(String),
    Num(f64),
}

impl From<&str> for Arg {
    fn from(v: &str) -> Self {
        Arg::Str(v.into())
    }
}
impl From<String> for Arg {
    fn from(v: String) -> Self {
        Arg::Str(v)
    }
}
impl From<usize> for Arg {
    fn from(v: usize) -> Self {
        Arg::Num(v as f64)
    }
}
impl From<u64> for Arg {
    fn from(v: u64) -> Self {
        Arg::Num(v as f64)
    }
}
impl From<i64> for Arg {
    fn from(v: i64) -> Self {
        Arg::Num(v as f64)
    }
}

type Bundle = FluentBundle<FluentResource>;

fn build(locale: Locale) -> Bundle {
    let lang: LanguageIdentifier = locale.tag().parse().expect("valid language tag");
    let mut bundle = FluentBundle::new_concurrent(vec![lang]);
    // Fluent wraps interpolated values in Unicode isolation marks, which show up in a WebView
    // and in tests; the UI never mixes directions inside a message.
    bundle.set_use_isolating(false);
    let resource = FluentResource::try_new(catalog_source(locale).to_string()).unwrap_or_else(
        |(resource, errors)| {
            tracing::error!(locale = locale.tag(), ?errors, "catalog has syntax errors");
            resource
        },
    );
    if let Err(errors) = bundle.add_resource(resource) {
        tracing::error!(locale = locale.tag(), ?errors, "catalog has duplicate ids");
    }
    bundle
}

fn bundle(locale: Locale) -> &'static Bundle {
    static BUNDLES: OnceLock<[Bundle; 3]> = OnceLock::new();
    let all = BUNDLES.get_or_init(|| Locale::ALL.map(build));
    &all[Locale::ALL.iter().position(|l| *l == locale).unwrap_or(1)]
}

fn format_in(locale: Locale, key: &str, args: Option<&FluentArgs>) -> Option<String> {
    let bundle = bundle(locale);
    let pattern = bundle.get_message(key)?.value()?;
    let mut errors = vec![];
    let text = bundle.format_pattern(pattern, args, &mut errors);
    if !errors.is_empty() {
        tracing::warn!(
            locale = locale.tag(),
            key,
            ?errors,
            "message formatting errors"
        );
    }
    Some(text.into_owned())
}

/// Translates `key` in `locale`; English when the locale lacks it; the id itself is never shown
/// (an unknown id yields an empty string and a warning — the catalog tests make this impossible
/// for ids used in the code).
pub fn tr(locale: Locale, key: &str) -> String {
    tr_args(locale, key, &[])
}

pub fn tr_args(locale: Locale, key: &str, args: &[(&str, Arg)]) -> String {
    let mut fluent = FluentArgs::new();
    for (name, value) in args {
        match value {
            Arg::Str(s) => fluent.set(*name, FluentValue::from(s.clone())),
            Arg::Num(n) => fluent.set(*name, FluentValue::from(*n)),
        }
    }
    let args = (!args.is_empty()).then_some(&fluent);
    format_in(locale, key, args)
        .or_else(|| format_in(Locale::En, key, args))
        .unwrap_or_else(|| {
            tracing::warn!(key, "missing translation key");
            String::new()
        })
}

/// Every message id of a locale's catalog (top-level `id = …` lines).
pub fn message_ids(locale: Locale) -> BTreeSet<String> {
    catalog_source(locale)
        .lines()
        .filter(|line| line.starts_with(|c: char| c.is_ascii_lowercase()))
        .filter_map(|line| line.split_once('=').map(|(id, _)| id.trim().to_string()))
        .collect()
}
