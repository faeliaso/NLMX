# ADR 0020 — Markdown in model answers: event renderer, HTML as text

Status: accepted

## Context

The answer renderer was hand-written (paragraphs, flat lists, bold, citation buttons): no italics,
code, tables, links, quotes or nested lists. Model output is untrusted text.

## Decision

- `ui-web/src/markdown.rs` parses with `pulldown-cmark` (already a workspace dependency, used by
  `parser-text`; no new crate, no JS library) and builds every tag itself from the parser's events
  (`push_html` is not used): cleanup (line endings, control characters) → parser (+
  `TextMergeStream`) → renderer → HTML.
- Raw HTML from the model is shown as escaped text; images become their alt text (nothing is
  loaded). Only `http`, `https` and `mailto` links become anchors (`is_safe_url`); any other scheme,
  relative or protocol-relative link is plain text. Anchors carry `data-external`; the click never
  navigates the WebView — `app.js` calls the Tauri command `open_external`, which validates the
  address again and runs `/usr/bin/open` (the address is never logged).
- Headings are rendered as `h3`–`h6` (the page owns `h1`/`h2`). Code blocks: `figure.code-block`
  with the language name (known names mapped, others sanitized, none = "Texto"), monospaced `pre`
  that scrolls horizontally, and a "Copiar" button (`data-copy-code`) that copies the code text.
  Tables: semantic `table` inside a focusable `.table-scroll` region. No syntax highlighting.
- Citation markers `[n]` / `[página N]` become buttons only in running text — never in code or
  links — with the same rules as before.
- "Copiar" on an answer copies the stored Markdown (plus the source list), not the HTML.
- Streaming is unchanged (plain text while generating; the Markdown fragment replaces it at
  `done`). `render` is pure and tolerant of partial text (tested), so rendering incrementally later
  only needs the renderer's output in `AnswerEvent`.
- User questions stay plain text (`white-space: pre-wrap`, escaped by the template).

## Consequences

Tests in `markdown.rs` cover the structures, malicious input, partial Markdown, UTF-8 and load. A
new tag or attribute must be added to the renderer explicitly; there is no pass-through.

## Update — syntax highlighting

Code blocks are coloured by `ui-web/src/highlight.rs`: a dependency-free, single-pass scanner per
language family (C-like, scripts, SQL, JSON/YAML, markup) that finds comments, strings, numbers,
keywords, calls, keys and tags. It is approximate by design (heredocs, interpolation and regex
literals may be coloured wrongly) and never rewrites text: the pieces concatenate back to the exact
code, so copying is unaffected. Unterminated strings or comments run to the end, so partial text
stays well formed. Output only uses the fixed classes `tok-kw|str|num|com|fn|key|tag|attr`
(colours: `--color-syntax-*` tokens). Unknown languages, Texto and Markdown are not highlighted, nor
is code above 100 KB. The language aliases live in `highlight::Lang::from_name`, shared with the
code block label.
