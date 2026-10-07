# ADR 0016 — Sources, preview and citation in the UI

- Status: accepted
- Date: 2026-10-04
- Addresses: the UI treats PDF, Markdown, TXT, CSV and EPUB as equivalent sources (continues ADR 0015). **Supersedes** the decision that citations of non-PDF formats are not clickable.

## Context
The RAG already returns provenance for any format, but the UI showed a single icon, non-PDF sources as inert items and had nowhere to say what a source is.

## Decision
- **Three separate things:** *Source* (every document: icon, name, format, status, information), *Preview* (PDF only, in the existing viewer) and *RAG citation* (links the two).
- **Single side panel.** The chat's `#viewer` slot receives the viewer (PDF) or **Informações da fonte** (Source information) (`GET /sources/{id}[?cite=…]`, `ui-web/src/sources.rs`): format, status, number of chunks, size, imported/indexed at, "usado em N conversas" (used in N conversations) (`DocumentRepository::source_details`), the cited excerpt when it came from an answer and, for PDF, the "Abrir no PDF" (Open in PDF) shortcut. There is no generic viewer and the content of Markdown/TXT/CSV/EPUB is never displayed. Opening, closing, Esc, focus and narrow mode are the viewer's.
- **Click.** In PDF it opens the viewer at the page; in any other format it selects the source and shows the information. `[n]` is always a button, with an `aria-label` that states the destination.
- **One place for formats.** `ui-web/src/formats.rs` maps `DocumentType` → icon and label; Documentos ("Detalhes" (Details) for non-PDF, `/chat?source=`), chat, Indexação and the panel use it. The indexing progress script does not know formats: it clones icons from a server-rendered `<template>`.
- **Stack.** HTMX 4 + askama + Tailwind 4 + tokens and plain JS; no TypeScript/Node (CLAUDE.md) nor Lucide dependency: the new icons are Lucide-style SVGs in `ui/components/icons`.

## Consequences
- A native `<select>` has no icons: the chat's scope selector shows the format as text.
- The number of conversations comes from citations marked as cited; sources only consulted do not count.
