use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use nlmx_i18n::{
    Arg, Locale, catalog_source, format_bytes, format_integer, format_percent, message_ids,
    resolve, tr, tr_args,
};

fn tags(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn normalizes_by_base_language() {
    for (raw, expected) in [
        ("pt-BR", Some(Locale::PtBr)),
        ("pt-PT", Some(Locale::PtBr)),
        ("pt_BR.UTF-8", Some(Locale::PtBr)),
        ("en-US", Some(Locale::En)),
        ("en-GB", Some(Locale::En)),
        ("en-AU", Some(Locale::En)),
        ("es-ES", Some(Locale::Es)),
        ("es-MX", Some(Locale::Es)),
        ("es-419", Some(Locale::Es)),
        ("fr-FR", None),
        ("de-DE", None),
        ("it-IT", None),
        ("ja-JP", None),
        ("zh-Hans-CN", None),
        ("", None),
    ] {
        assert_eq!(Locale::normalize(raw), expected, "{raw}");
    }
}

#[test]
fn resolution_priority_is_saved_then_system_then_english() {
    assert_eq!(resolve(Some(Locale::En), &tags(&["pt-BR"])), Locale::En);
    assert_eq!(resolve(None, &tags(&["pt-PT"])), Locale::PtBr);
    assert_eq!(resolve(None, &tags(&["es-MX", "en"])), Locale::Es);
    assert_eq!(resolve(None, &tags(&["fr-FR"])), Locale::En);
    // Only the first system language counts: no "similar language" guessing.
    assert_eq!(resolve(None, &tags(&["fr-FR", "pt-BR"])), Locale::En);
    assert_eq!(resolve(None, &[]), Locale::En);
}

#[test]
fn tags_round_trip() {
    for locale in Locale::ALL {
        assert_eq!(Locale::from_tag(locale.tag()), Some(locale));
    }
    assert_eq!(Locale::from_tag("pt"), None);
}

#[test]
fn catalogs_have_the_same_ids() {
    let reference = message_ids(Locale::En);
    assert!(!reference.is_empty());
    for locale in [Locale::PtBr, Locale::Es] {
        let ids = message_ids(locale);
        let missing: Vec<_> = reference.difference(&ids).collect();
        let extra: Vec<_> = ids.difference(&reference).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "{}: missing {missing:?}, extra {extra:?}",
            locale.tag()
        );
    }
}

/// `$variables` used by each message (a message may span indented continuation lines).
fn variables(locale: Locale) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in catalog_source(locale).lines() {
        if line.starts_with(|c: char| c.is_ascii_lowercase()) {
            let id = line.split('=').next().unwrap().trim().to_string();
            out.entry(id.clone()).or_default();
            current = Some(id);
        } else if !line.starts_with('#') && !line.trim().is_empty() {
            // continuation line
        } else if line.trim().is_empty() {
            current = None;
        }
        if let Some(id) = &current {
            let mut rest = line;
            while let Some(i) = rest.find('$') {
                let name: String = rest[i + 1..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .collect();
                out.get_mut(id).unwrap().insert(name);
                rest = &rest[i + 1..];
            }
        }
    }
    out
}

#[test]
fn catalogs_have_no_duplicate_ids() {
    for locale in Locale::ALL {
        let mut seen = BTreeSet::new();
        for line in catalog_source(locale).lines() {
            if line.starts_with(|c: char| c.is_ascii_lowercase()) {
                let id = line.split('=').next().unwrap().trim();
                assert!(
                    seen.insert(id.to_string()),
                    "{} / duplicate id {id}",
                    locale.tag()
                );
            }
        }
    }
}

#[test]
fn catalogs_use_the_same_variables() {
    let reference = variables(Locale::En);
    for locale in [Locale::PtBr, Locale::Es] {
        let other = variables(locale);
        for (id, vars) in &reference {
            assert_eq!(other.get(id), Some(vars), "{} / {id}", locale.tag());
        }
    }
}

#[test]
fn catalogs_have_no_syntax_errors_or_empty_messages() {
    for locale in Locale::ALL {
        for id in message_ids(locale) {
            // Every id must format in its own locale without falling back.
            let text = tr_args(
                locale,
                &id,
                &[("count", Arg::Num(2.0)), ("name", "x".into())],
            );
            assert!(!text.is_empty(), "{} / {id} is empty", locale.tag());
        }
    }
}

#[test]
fn missing_keys_never_show_the_id() {
    assert_eq!(tr(Locale::Es, "does-not-exist"), "");
}

#[test]
fn interpolation_and_plurals_per_language() {
    // `test-plural` lives only in the tests' own expectations through `common-items`.
    for (locale, one, many) in [
        (Locale::En, "1 item", "3 items"),
        (Locale::PtBr, "1 item", "3 itens"),
        (Locale::Es, "1 elemento", "3 elementos"),
    ] {
        assert_eq!(
            tr_args(locale, "common-items", &[("count", 1usize.into())]),
            one
        );
        assert_eq!(
            tr_args(locale, "common-items", &[("count", 3usize.into())]),
            many
        );
    }
}

#[test]
fn formats_numbers_per_locale() {
    assert_eq!(format_bytes(Locale::En, 1_500_000), "1.5 MB");
    assert_eq!(format_bytes(Locale::PtBr, 1_500_000), "1,5 MB");
    assert_eq!(format_bytes(Locale::Es, 812), "812 B");
    assert_eq!(format_percent(Locale::En, 0.5), "50%");
    assert_eq!(format_percent(Locale::Es, 0.5), "50 %");
    assert_eq!(format_integer(Locale::PtBr, 8192), "8.192");
    assert_eq!(format_integer(Locale::En, 8192), "8,192");
    assert_eq!(format_integer(Locale::Es, 1_234_567), "1.234.567");
    assert_eq!(format_integer(Locale::En, 0), "0");
    assert_eq!(format_integer(Locale::PtBr, 999), "999");
    assert_eq!(format_integer(Locale::PtBr, 1000), "1.000");
}

fn rust_and_template_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap().to_string_lossy();
            if name != "vendor" && name != "target" {
                rust_and_template_files(&path, out);
            }
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs" | "html")
        ) {
            out.push(path);
        }
    }
}

/// Every id referenced as `t("…")`, `t_args("…"`, `t_count("…"` in the UI sources exists.
#[test]
fn ids_used_in_the_code_exist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = vec![];
    for dir in [
        "crates/ui-web/src",
        "apps/desktop/ui",
        "apps/desktop/src-tauri/src",
    ] {
        rust_and_template_files(&root.join(dir), &mut files);
    }
    let known = message_ids(Locale::En);
    let mut unknown = vec![];
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        for pattern in ["t(\"", "t_args(\"", "t_count(\""] {
            let mut rest = text.as_str();
            while let Some(i) = rest.find(pattern) {
                let before = rest[..i].chars().last();
                let after = &rest[i + pattern.len()..];
                let id: String = after
                    .chars()
                    .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
                    .collect();
                let standalone =
                    !before.is_some_and(|c| c.is_alphanumeric() || c == '_') || before == Some(':');
                if standalone && after[id.len()..].starts_with('"') && !known.contains(&id) {
                    unknown.push(format!("{} → {id}", file.display()));
                }
                rest = after;
            }
        }
    }
    assert!(
        unknown.is_empty(),
        "ids missing from the catalog: {unknown:#?}"
    );
}
