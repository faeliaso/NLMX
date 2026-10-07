# 0022 — Interface internationalization with Project Fluent

## Status
Accepted.

## Context
The interface was hardcoded in Portuguese. It must exist in pt-BR, English and Spanish, follow the macOS language on first run, let the user choose another language, switch without restarting, and be ready for more languages. The UI is HTML rendered in Rust (askama + axum, in-process) with HTMX and plain JS; the project has no Node/TypeScript (so i18next and similar are out), and the answer language of the model is a separate matter.

## Decision
- Library: **Project Fluent** (`fluent-bundle` + `unic-langid`) in a new crate `crates/i18n` (`nlmx-i18n`). Pure Rust, CLDR plural rules, interpolation, English fallback, catalogs embedded at build time. `sys-locale` reads the macOS preferred languages (in `src-tauri` only). `domain` and `application` do not depend on it.
- Catalogs: `crates/i18n/locales/<tag>/<namespace>.ftl` for `pt-BR`, `en`, `es`; namespaces `common nav status fm documents sources chat viewer models indexing settings diagnostics errors js`. Ids are semantic and start with their namespace (`chat-send`). Tests require the same ids and the same variables in the three locales and that every id used in the UI sources exists.
- Resolution: saved choice → first macOS language, normalized by base language (`pt*`→pt-BR, `en*`→en, `es*`→es) → **English**. No regional guessing; Portuguese is never the universal fallback. Only the first system language counts.
- Persistence: `app_settings` key `ui.language` (`"pt-BR"|"en"|"es"`), written only when the user chooses. The saved choice wins over the system language forever.
- The locale is process-wide (single-user desktop app): `nlmx_i18n::current()`. A middleware in `ui-web` resolves it before the first request. Templates call `{{ nlmx_i18n::t("id") }}`; Rust code calls `t`, `t_args`, `t_count`; sizes and percentages use `format_bytes`/`format_percent`.
- Switching: Configurações → Idioma posts `/settings/language`; the response triggers `language-changed`, and `app.js` reloads the current page content and `<html lang>` without reloading the window. Strings needed by JS come from the `i18n-js` JSON block of the layout.
- The model's answer language is independent: `domain::generation::ResponseLanguage::Auto` (default) adds one "RESPONSE LANGUAGE: AUTO" instruction to the RAG, free-chat and rewrite prompts. It is never derived from the interface language. The persisted not-found sentence is shown localized at display time.
- Not translated: documents, notes, file names, model output, product names, technical logs and error diagnostics.

## Consequences
- Adding a language: create `locales/<tag>/*.ftl` (copy `en`), add the variant to `Locale` and the files to `catalog_files!`, run `cargo test -p nlmx-i18n`.
- Every user-visible string goes through the catalog; the parity tests catch missing ids and variables.
- Documents saved before this change that contain the old Portuguese not-found sentence are still recognised.
