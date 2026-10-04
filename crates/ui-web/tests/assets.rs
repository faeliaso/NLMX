//! Consistency of the front-end assets that nothing else compiles: every icon a template names
//! exists, every format has its icon, styles use only tokens, and the scripts parse.

use std::{
    fs,
    path::{Path, PathBuf},
};

fn ui() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/ui")
}

fn files(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "vendor") {
                continue;
            }
            files(&path, extension, out);
        } else if path.extension().is_some_and(|e| e == extension) {
            out.push(path);
        }
    }
}

fn all(extension: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    files(&ui(), extension, &mut out);
    out
}

/// Icon names written literally in templates: `ds::icon("name")`, `icon("name")`.
fn literal_icons(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("icon(\"") {
        let after = &rest[at + 6..];
        if let Some(end) = after.find('"') {
            names.push(after[..end].to_string());
        }
        rest = after;
    }
    names
}

#[test]
fn every_icon_a_template_names_exists() {
    let macro_source = fs::read_to_string(ui().join("components/ds.html")).unwrap();
    let mut checked = 0;
    for template in all("html") {
        let source = fs::read_to_string(&template).unwrap();
        for name in literal_icons(&source) {
            // The macro maps the name to its file, and the file exists.
            assert!(
                macro_source.contains(&format!("name == \"{name}\"")),
                "{}: icon {name:?} is not in the ds::icon macro",
                template.display()
            );
            checked += 1;
        }
    }
    assert!(checked > 40, "the scan found the icons ({checked})");
    // Every file the macro includes is there.
    for line in macro_source.lines() {
        if let Some(at) = line.find("include \"icons/") {
            let name = line[at + 9..].split('"').next().unwrap();
            assert!(
                ui().join("components").join(name).exists(),
                "missing {name}"
            );
        }
    }
}

#[test]
fn every_icon_is_a_well_formed_svg_that_follows_the_theme() {
    let mut icons = Vec::new();
    files(&ui().join("components/icons"), "html", &mut icons);
    assert!(icons.len() >= 35);
    for icon in icons {
        let svg = fs::read_to_string(&icon).unwrap();
        assert!(
            svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"),
            "{}",
            icon.display()
        );
        assert!(
            svg.contains("currentColor"),
            "{}: colors come from the theme",
            icon.display()
        );
        assert!(svg.contains("aria-hidden=\"true\""), "{}", icon.display());
        assert!(
            !svg.contains("fill=\"#") && !svg.contains("stroke=\"#"),
            "{}",
            icon.display()
        );
    }
}

#[test]
fn styles_use_tokens_only() {
    for css in all("css") {
        if css.file_name().is_some_and(|n| n == "tokens.css") {
            continue;
        }
        let source = fs::read_to_string(&css).unwrap();
        for (i, line) in source.lines().enumerate() {
            let code = line.split("/*").next().unwrap_or("");
            // The one intentional exception since the first version: the switch knob is white
            // in both themes.
            if css.ends_with("field.css") && code.contains("background: #FFFFFF;") {
                continue;
            }
            let raw_color = code.contains("rgb(")
                || code.contains("rgba(")
                || code.contains("hsl(")
                || code.split('#').skip(1).any(|t| {
                    let hex: String = t.chars().take_while(char::is_ascii_hexdigit).collect();
                    matches!(hex.len(), 3 | 6 | 8)
                        && !t[hex.len()..]
                            .starts_with(|c: char| c.is_alphanumeric() || c == '-' || c == '_')
                        && !code.contains("url(")
                });
            assert!(
                !raw_color,
                "{}:{}: a raw color — use a token: {line}",
                css.display(),
                i + 1
            );
            assert!(
                !code.contains("dark:"),
                "{}:{}: no dark: variants",
                css.display(),
                i + 1
            );
        }
    }
}

#[test]
fn scripts_have_no_syntax_errors_when_a_checker_is_around() {
    // Development only: the app ships plain scripts and no Node. If `node` is installed, parse them.
    let Ok(version) = std::process::Command::new("node").arg("--version").output() else {
        eprintln!("node is not installed: skipping the syntax check of the scripts");
        return;
    };
    assert!(version.status.success());
    for script in all("js") {
        let output = std::process::Command::new("node")
            .arg("--check")
            .arg(&script)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: {}",
            script.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
