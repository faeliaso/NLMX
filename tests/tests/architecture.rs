//! Enforces the dependency rule from docs/adr/0001 on the real Cargo graph.

use std::collections::{BTreeMap, BTreeSet};

use cargo_metadata::{DependencyKind, MetadataCommand};

const DOMAIN: &str = "nlmx-domain";
const APPLICATION: &str = "nlmx-application";
const UI_WEB: &str = "nlmx-ui-web";

/// Infrastructure libraries that must never reach the core layers.
const INFRASTRUCTURE: &[&str] = &[
    "tauri",
    "rusqlite",
    "sqlite-vec",
    "pdfium-render",
    "llama-cpp-2",
    "hyper",
    "hyperlocal",
    "axum",
    "askama",
    "reqwest",
];

struct Workspace {
    /// Normal (non-dev, non-build) dependencies of every workspace crate, by crate name.
    deps: BTreeMap<String, BTreeSet<String>>,
    /// Crates under `crates/adapters/`.
    adapters: BTreeSet<String>,
}

fn workspace() -> Workspace {
    let metadata = MetadataCommand::new()
        .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/../Cargo.toml"))
        .no_deps()
        .exec()
        .expect("cargo metadata");

    let mut deps = BTreeMap::new();
    let mut adapters = BTreeSet::new();
    for package in metadata.workspace_packages() {
        let name = package.name.to_string();
        if package.manifest_path.as_str().contains("/crates/adapters/") {
            adapters.insert(name.clone());
        }
        let normal = package
            .dependencies
            .iter()
            .filter(|dep| dep.kind == DependencyKind::Normal)
            .map(|dep| dep.name.clone())
            .collect();
        deps.insert(name, normal);
    }
    Workspace { deps, adapters }
}

#[test]
fn adapters_are_discovered() {
    assert_eq!(workspace().adapters.len(), 13, "expected 13 adapter crates");
}

#[test]
fn domain_has_no_workspace_or_infrastructure_dependencies() {
    let graph = workspace().deps;
    let violations: Vec<_> = graph[DOMAIN]
        .iter()
        .filter(|dep| dep.starts_with("nlmx-") || INFRASTRUCTURE.contains(&dep.as_str()))
        .collect();
    assert!(
        violations.is_empty(),
        "{DOMAIN} must be pure, found {violations:?}"
    );
}

#[test]
fn application_depends_only_on_domain_among_workspace_crates() {
    let graph = workspace().deps;
    let violations: Vec<_> = graph[APPLICATION]
        .iter()
        .filter(|dep| {
            (dep.starts_with("nlmx-") && dep.as_str() != DOMAIN)
                || INFRASTRUCTURE.contains(&dep.as_str())
        })
        .collect();
    assert!(
        violations.is_empty(),
        "{APPLICATION} has forbidden deps {violations:?}"
    );
}

#[test]
fn adapters_do_not_depend_on_each_other() {
    let Workspace {
        deps: graph,
        adapters,
    } = workspace();
    for adapter in &adapters {
        let violations: Vec<_> = graph[adapter].intersection(&adapters).collect();
        assert!(
            violations.is_empty(),
            "{adapter} depends on adapters {violations:?}"
        );
    }
}

#[test]
fn ui_web_does_not_depend_on_adapters_or_tauri() {
    let Workspace {
        deps: graph,
        adapters,
    } = workspace();
    let violations: Vec<_> = graph[UI_WEB]
        .iter()
        .filter(|dep| adapters.contains(*dep) || dep.as_str() == "tauri")
        .collect();
    assert!(
        violations.is_empty(),
        "{UI_WEB} has forbidden deps {violations:?}"
    );
}

#[test]
fn only_the_composition_root_uses_the_foundation_models_adapter() {
    let graph = workspace().deps;
    let users: Vec<_> = graph
        .iter()
        .filter(|(_, deps)| deps.contains("nlmx-llm-fm"))
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(
        users,
        ["nlmx-desktop"],
        "the RAG only sees the LlmProvider port"
    );
}

#[test]
fn the_rag_core_knows_nothing_about_apple_foundation_models() {
    // The application layer talks to `LlmProvider`; the `fm` tool is an adapter detail.
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../crates/application/src");
    let mut stack = vec![std::path::PathBuf::from(root)];
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            stack.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for needle in ["/usr/bin/fm", "fm serve", "FoundationModels", "nlmx_llm_fm"] {
            assert!(
                !text.contains(needle),
                "{} mentions {needle:?}",
                path.display()
            );
        }
    }
}

#[test]
fn the_content_pipeline_stops_before_embeddings() {
    // parse → normalize → chunk. Embeddings are the next stage, fed by stored chunks; the
    // pipeline and the ports of its stages must not know about them.
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../crates/application/src/services/pipeline.rs"
    ))
    .unwrap();
    for needle in ["Embedding", "embedding", "EmbedDocuments", "VectorStore"] {
        // The module doc names the stage that follows; code must not use it.
        let code: String = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!code.contains(needle), "pipeline.rs uses {needle:?}");
    }
}

#[test]
fn downloads_are_confirmed_only_by_the_download_command() {
    // `DownloadPlan::confirm()` must only be called from the user's explicit action.
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
    let mut found = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = ["crates", "apps"]
        .iter()
        .map(|d| std::path::Path::new(root).join(d))
        .collect();
    while let Some(path) = stack.pop() {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if path.is_dir() {
            if !matches!(name, "tests" | "target" | "node_modules") {
                stack.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
            }
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        // Production code only: stop at the unit-test module.
        let production = text.split("#[cfg(test)]").next().unwrap_or_default();
        if production.contains(".confirm()") {
            found.push(path.strip_prefix(root).unwrap().display().to_string());
        }
    }
    assert_eq!(
        found,
        ["apps/desktop/src-tauri/src/commands.rs"],
        "{found:?}"
    );
}
