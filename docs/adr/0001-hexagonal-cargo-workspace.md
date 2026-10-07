# ADR 0001 — Hexagonal architecture in a Cargo workspace

- Status: accepted
- Date: 2026-10-02

## Context
The app combines several native libraries (PDFium, llama.cpp, SQLite/sqlite-vec) and an external process (`fm`), all of which may be replaced in the future. Ingestion, search and RAG logic must be testable without these dependencies, and swapping one of them must not spread through the code.

## Decision
Ports & Adapters, with one crate per layer/adapter in a Cargo workspace:
`domain` ← `application` (use cases + ports) ← `adapters/*` and `ui-web` ← `app-tauri` (composition root).
Adapters never depend on each other; `application` does not import infrastructure libraries; external types do not cross ports. Fakes and per-port contract suites live in `testing/`.

## Consequences
- Boundaries enforced by the compiler, not by convention.
- Use cases testable with fakes, without GPU, PDFium or `fm`.
- Cost: more crates, type mapping in each adapter, explicit wiring in `app-tauri`.
