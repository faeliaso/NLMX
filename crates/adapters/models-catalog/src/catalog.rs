//! The model catalog embedded in the app. Updated with app releases (no remote catalog).

use nlmx_domain::models::{License, ModelDescriptor};
use serde::Deserialize;

const BUILTIN: &str = include_str!("../catalog/models.json");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    id: String,
    display_name: String,
    description: String,
    version: String,
    url: String,
    file_name: String,
    size: u64,
    sha256: String,
    license: LicenseEntry,
    languages: Vec<String>,
    dimensions: u32,
    pooling: String,
    query_prefix: String,
    passage_prefix: String,
    context_size: u32,
    recommended: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LicenseEntry {
    id: String,
    url: String,
}

/// Parses a catalog document (the embedded one, or one supplied in tests).
pub fn parse(json: &str) -> Result<Vec<ModelDescriptor>, String> {
    let entries: Vec<Entry> =
        serde_json::from_str(json).map_err(|err| format!("catálogo inválido: {err}"))?;
    let models: Vec<ModelDescriptor> = entries
        .into_iter()
        .map(|e| ModelDescriptor {
            id: e.id,
            display_name: e.display_name,
            description: e.description,
            version: e.version,
            url: e.url,
            file_name: e.file_name,
            size: e.size,
            sha256: e.sha256.to_lowercase(),
            license: License {
                id: e.license.id,
                url: e.license.url,
            },
            languages: e.languages,
            dimensions: e.dimensions,
            pooling: e.pooling,
            query_prefix: e.query_prefix,
            passage_prefix: e.passage_prefix,
            context_size: e.context_size,
            recommended: e.recommended,
        })
        .collect();
    validate(&models)?;
    Ok(models)
}

pub fn builtin() -> Vec<ModelDescriptor> {
    parse(BUILTIN).expect("embedded catalog is valid (checked by tests)")
}

fn validate(models: &[ModelDescriptor]) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    for m in models {
        if !ids.insert(&m.id) {
            return Err(format!("id duplicado no catálogo: {}", m.id));
        }
        let safe = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        };
        if !safe(&m.id) || !safe(&m.version) || !safe(&m.file_name) {
            return Err(format!(
                "{}: id, versão e arquivo devem ser nomes simples (sem barras)",
                m.id
            ));
        }
        if m.sha256.len() != 64 || !m.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("{}: SHA-256 inválido", m.id));
        }
        if !m.file_name.ends_with(".gguf") || m.size == 0 || m.dimensions == 0 {
            return Err(format!("{}: arquivo, tamanho ou dimensões inválidos", m.id));
        }
    }
    Ok(())
}
