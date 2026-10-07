# NLMX — Architecture

How the system is today. Vision and requirements: [`PRODUCT.md`](PRODUCT.md). Decisions: [`adr/`](adr/). Day-to-day rules for anyone editing the code: `CLAUDE.md` at the root.

## 1. Style and dependency rules

**Hexagonal (Ports & Adapters) in a Cargo workspace** (ADR 0001): each layer is a crate, so the dependency rule is enforced by the compiler and checked by `tests/tests/architecture.rs` against the real `cargo metadata` graph.

```
                 apps/desktop/src-tauri  (nlmx-desktop: composition root + shell)
                    │                │
          crates/ui-web        crates/adapters/*
                    │                │   implement ports
                    └──► crates/application ◄──┘   use cases + services + ports (traits)
                                  │
                           crates/domain          entities, value objects, pure functions
```

- `domain` depends on nothing external; `application` depends only on `domain` (never on rusqlite, pdfium-render, tauri, axum, askama…).
- **Adapters never depend on each other**; `ui-web` knows neither adapters nor Tauri.
- No external type crosses a port: adapters map to `domain` types and to the port's error enum.
- Only `src-tauri/src/wiring.rs` instantiates adapters (DI with `Arc<dyn Port>`).
- Every port has an in-memory fake and a reusable contract suite in `crates/testing`, run against both the fake and the real adapter.

## 2. Layout

```
crates/
  domain/                 pure types and functions: document, ingestion, retrieval (fuse, join_adjacent,
                          jaccard…), rag_intent, viewer (find_in_spans), telemetry (Measurement)
  application/            ports.rs · use_cases/ (DocumentIngestion, EmbedDocuments, ChatService,
                          ViewDocument, GetSystemStatus) · services/ (retrieval, retriever, rag/, free_chat)
  ui-web/                 in-process axum router, handlers, safe markdown, view models
  testing/                fakes for all ports + contract suites
  adapters/
    pdf-pdfium/           DocumentEngine (single PDFium thread)
    structure-heuristic/  StructureAnalyzer
    chunker-structural/   Chunker
    embed-llama/          EmbeddingProvider + InferenceRuntime (supervised llama-server)
    store-sqlite/         repositories, LexicalIndex (FTS5), VectorStore (sqlite-vec), migrations
    llm-fm/               LlmProvider (fm serve / fm respond)
    models-catalog/       ModelProvider (catalog embedded in catalog/models.json, download)
    fs-library/           FileStore (PDF library, SHA-256)
    telemetry/            JSON logs with redaction, MetricsRegistry, memory/disk sampler
apps/desktop/
  src-tauri/              main, wiring, protocol (nlmx://), commands, tauri.conf.json
  ui/                     askama templates (pages, components), styles (Tailwind 4), scripts (minimal JS)
tests/                    workspace tests: architecture, model activation, notes, release acceptance
```

## 3. Ports (`crates/application/src/ports.rs`)

| Port | Purpose | Adapter |
|---|---|---|
| `DocumentEngine` | open PDF, metadata, text spans with bbox, images, render page | `pdf-pdfium` |
| `DocumentParser` | read one format (PDF, Markdown, TXT, CSV/TSV, EPUB, DOCX, XLSX) into a `ParsedDocument`: metadata, sections, typed blocks and location; per-format registry in `ParserRegistry` (not yet wired into ingestion, ADR 0011) | `PdfDocumentParser` (in `application`, on top of `DocumentEngine` + `StructureAnalyzer`), `parser-text` (Markdown, TXT, CSV), `parser-epub`, `parser-office` (DOCX and XLSX, ADR 0017) |
| `DocumentNormalizer` · `DocumentChunker` | clean the text of a `ParsedDocument` and cut it into `DocumentChunk`s that keep the section path and `SourceLocation`; composed with the parser by `ContentPipeline` (parse → normalize → chunk, no embeddings; ADR 0012; wired into ingestion by ADR 0014) | `normalizer-text`, `chunker-structural` (`MultiFormatChunker`) |
| `FileStore` | hash and copy the PDF into the library | `fs-library` |
| `StructureAnalyzer` · `Chunker` · `TokenCounter` | layout → sections/blocks → chunks | `structure-heuristic`, `chunker-structural` |
| `DocumentRepository` | documents, pages, atomic `save_extraction`, pending items | `store-sqlite` |
| `IndexingReader` | read-only: documents with their latest ingestion job and vectors per model (Indexação screen) | `store-sqlite` |
| `ChunkReader` | resolve filters, read the chunks of a document or by ids | `store-sqlite` |
| `LexicalIndex` · `VectorStore` | BM25 (FTS5) and KNN (vec0), kept separate even in the same database | `store-sqlite` |
| `ConversationRepository` | conversations, scope, messages, sources/citations | `store-sqlite` |
| `SettingsRepository` · `StorageDiagnostics` | settings and on-disk sizes | `store-sqlite` |
| `EmbeddingProvider` · `EmbeddingSource` | Query/Passage vectors; current provider (swappable at runtime) | `embed-llama`; `EmbeddingSlot` in the wiring |
| `InferenceRuntime` | version/path of the bundled llama.cpp | `embed-llama` |
| `ModelProvider` | catalog, plan + confirmed download, verify, activate, remove, update | `models-catalog` |
| `LlmProvider` | status, exact token count, streaming generation with cancellation | `llm-fm` |
| `Diagnostics` | aggregated session metrics | `telemetry` |

Streaming and progress leave `application` through callbacks (`on_token`, download progress) that the shell translates into Tauri Channels/Events.

## 4. Ingestion

```
picker ─► import_documents (Tauri) ─► DocumentIngestion::import(path)
  SHA-256 ─► duplicate? (failed/unfinished ⇒ reprocess) ─► FileStore: <data>/library/<sha>.pdf
  ─► ingest(id): DocumentEngine ─► StructureAnalyzer ─► Chunker
  ─► DocumentRepository::save_extraction (pages + chunks + FTS in one transaction)
  ─► EmbedDocuments: EmbeddingProvider(Passage) ─► VectorStore (vector + chunk_embeddings in one transaction)
```

- Document status: `queued → extracting → structuring → chunking → embedding → indexed`, or `needs_ocr` (no text layer) / `failed` (corrupted, password). A failure never affects other documents.
- Nothing is written before `save_extraction`, so retrying is idempotent; at boot, `resume()` reprocesses stalled documents and `embed_pending()` generates the missing vectors.
- Without an active embedding model, the document stays in `embedding` (job `waiting_model`) and search is lexical-only.
- A change in the analyzer's or chunker's output ⇒ bump `structure_heuristic::VERSION` / `chunker_structural::VERSION`.

### Notes (ADR 0018)

```
"Adicionar nota" (dialog) ─► POST /documents/notes ─► NoteSubmitter (NoteInbox) ─► ImportQueue::push_note
  clean_note_text (empty/whitespace-only ⇒ rejected) ─► DocumentIngestion::enqueue_note
  ─► digest_text (duplicate) ─► documents (format 'note', note_text, no file) ─► queued
  ─► ingest(id): NoteDocumentParser(DocumentSource::note(text)) ─► normalizer ─► MultiFormatChunker
  ─► save_processed (SourceLocation::Note) ─► EmbedDocuments ─► VectorStore ─► RAG
```

It is the same pipeline and the same queue as files; only the source of the text changes (database instead of `<sha>.<ext>`). Reindexing and resuming read `note_text`.

### Indexing (screen)

```
GET /indexing · /fragments/indexing ─► Indexing::report ─► IndexingReader::snapshot(active model)
  ─► IndexingModel: index state, "Em andamento", "Precisa de atenção", "Concluídos recentemente"
buttons ─► retry_document · retry_failed · embed_pending_now · reindex_all (Tauri)
  ─► Indexing::begin (exclusive; refuses with `busy`) ─► background task ─► `indexing-changed` event
```

(The section titles above are the screen's labels: in progress, needs attention, recently completed.)

- `IndexingActivity` counts background work: imports, `resume`/`embed_pending` at boot and `apply_model_change` call `begin()` (they may overlap); the screen's actions use `try_begin()` (they never overlap with anything).
- While there is work (activity or a document being read), the fragment renders with `hx-trigger="every 2s, …"`; with no work, it only reacts to `indexing-changed`/`documents-changed`/`models-changed`. The element has no `id` (HTMX 4 settle quirk) and has `data-poll` (it does not trigger the global loading indicator).
- "Tentar novamente" (retry): `failed` ⇒ `DocumentIngestion::retry` (needs PDFium); `embedding` ⇒ `EmbedDocuments::embed_document`.
- Duration of a job = `finished_at − started_at` of `embedding_jobs` (the job mirrors the document status).

### Removal (ADR 0008)

```
Documentos ─► "Remover…" menu ─► dialog (RemoveDocument::impact) ─► POST /documents/{id}/delete
  ─► RemoveDocument::remove: refuses if queued/extracting/structuring/chunking
  ─► DocumentRepository::remove (one transaction): conversations scoped to the document; question +
     answer pairs that used it; conversations left empty; the document (cascades: pages,
     chunks, FTS5, vec0, chunk_embeddings, jobs, collections, citations) ─► wal_checkpoint(TRUNCATE)
  ─► FileStore::remove(<sha>.pdf) ─► ViewDocument::forget(id) ─► Measurement::DocumentRemoved
```

- `secure_delete` (connection) and FTS5 `secure-delete` (migration 0009) keep the removed text out of the database bytes.
- At startup, `RemoveDocument::prune_library` deletes library files that have no document (before `resume()`).

## 5. Search

**`HybridRetriever`** (`application::services::retrieval`), with `RetrievalOptions { top_k, semantic_weight, lexical_weight, filter }`:

1. Filters (document, collection, page) resolved by `ChunkReader::resolve`.
2. Vector: `embed(Query)` → `VectorStore::search` (cosine KNN, `document_id IN` filter inside vec0).
3. Lexical: `LexicalQuery` (lowercase terms, no PT/EN stopwords, each term quoted — user text never goes raw into the `MATCH`) → `LexicalIndex::search` (BM25).
4. Weighted fusion (`domain::retrieval::fuse`, ADR 0007): cosine clamped to [0, 1] + min-max of −bm25 × term coverage²; ties broken by the semantic score and then by id.
5. Without a model (or if it fails) it falls back to lexical and reports `mode` + `warnings`.

**`Retriever`** (`application::services::retriever`) prepares the RAG context: it oversamples (`top_k × 3`), removes exact duplicates across documents (`metadata.duplicates`), joins neighbouring chunks **of the same section** without repeating the overlap, removes near-duplicates (trigram Jaccard ≥ 0.8 **and** same numbers), and applies `min_score`, `max_per_document` and Top-K.

## 6. Question and answer

```
POST /chat/{id}/messages ─► question + "streaming" answer saved ─► HTML turn
app.js ─► command answer_message({messageId, onEvent: Channel})
        ─► ChatService::answer ─► RagEngine (document mode) | FreeChat (free) ─► {kind:"token"}… {kind:"done"}
done ─► GET /chat/messages/{id} (final HTML)      cancel_answer ─► CancelFlag (partial answer saved)
```

**Scope and mode (ADR 0009).** A conversation is free (default), about all documents, or about one document (`ConversationScope`). Each answer stores the mode it was generated in (`messages.grounding`), and `ChatService::answer` chooses by it:

- **Free** — `FreeChat` (`application::services::free_chat`) receives only the `LlmProvider`, with no access to the library. Fixed instructions go in `system`. The last 6 turns (each up to 1,500 characters, neutralized) go in `GenerationRequest::history`, which `fm serve` receives as `user`/`assistant` messages; `user` carries only the question. `fm respond` and `fm count-tokens` receive `flat_user()`. Older turns are dropped until `count_tokens` fits. "Explique este documento." (explain this document) asks for a document without calling the model. The answer has no sources.
- **Document** — `RagEngine`, below. A `NotFound` answer can be redone without the documents (`POST /chat/messages/{id}/free` → `ChatService::answer_freely`), and becomes free.

`RagEngine::ask` (`application::services::rag`):

1. **Intent** (`domain::rag_intent`): ordinary question → search; "Explique este documento." → `Overview` (first passage of each section of the document in scope); "seção N" (section N) → `Section(N)` (passages of the section and its subsections, complemented by search; with no matching heading, regular search). With history, the follow-up is rewritten as a standalone question by a short LLM call.
2. **Retriever** + **relevance gate**: best score < `min_relevance` (0.35) ⇒ `NotFound` **without calling the model**, showing the best passages. (Overview and Section skip the gate.)
3. **`ContextBuilder`** (`rag/context.rs`, pure): budget = min(1,800, 4,096 window − instructions − question − 700 answer reserve − 10% margin); passages numbered in `<trecho>` blocks, documents by relevance and pages in order; duplicates and contained passages dropped.
4. **Isolation**: fixed instructions only in `system`; document text and question only in `user`, passed through `context::neutralize` (`<`/`>` → `‹`/`›`, no control characters).
5. **Exact count** with `count_tokens` (`fm count-tokens`); above the limit, it removes the weakest passage and recomputes. `fm serve` does not reject a long prompt — it degenerates — so the limit is enforced here.
6. **Generation** with streaming; guardrail refusal ⇒ `Refused` with the passages.
7. **`CitationEngine`** (`rag/citations.rs`, pure): `[n]`, `[1, 3]`, `[2–3]` → document/chunk/pages/bboxes; invalid numbers removed; `[página N]` (page N) resolved to the source that covers the page. All the sources sent are kept in `citations` (`cited` marks the cited ones); page references in `message_page_refs`.

Model output is rendered only by `ui-web/src/markdown.rs` (ADR 0020): `pulldown-cmark` events → allow-listed HTML (headings, lists, emphasis, quotes, code blocks with copy button and approximate syntax highlighting (`highlight.rs`), tables, safe links, citation buttons); raw HTML is escaped as text, images are dropped, links open through the `open_external` command.

## 7. PDF Viewer

Everything through PDFium, no pdf.js. `GET /viewer/{doc}?page=N&cite={msg}-{n}` or `&ref={msg}-{page}` opens beside the chat (resizable, full screen, Esc closes); `/chat?view={doc}` opens it from Documentos.

- Page sizes come from the database (`document_pages`), without opening the PDF.
- `ui/scripts/viewer.js` loads only the pages near the viewport: PNG in width steps (`/documents/{id}/pages/{n}.png?w=`) and a text layer (`/viewer/{doc}/pages/{n}/text`, transparent spans for native selection).
- Search `/viewer/{doc}/search?q=` (`domain::viewer::find_in_spans`: accent/case-insensitive, crosses spans and hyphenation); spans cached per page in `ViewDocument`.
- Highlights are boxes in PDF points (top-left origin) converted to % of the page. Zoom 50–300%, thumbnails, ←/→ shortcuts, ⌘+/−/0, ⌘F.

## 8. Processes and threads

| Unit | Type | Owner | Lifecycle |
|---|---|---|---|
| UI / event loop | main thread | Tauri | app |
| Async runtime | tokio multi-thread | Tauri | app |
| PDFium | 1 dedicated thread (not thread-safe) | `pdf-pdfium` | lazy |
| `llama-server` | child process (sidecar), 127.0.0.1, ephemeral port, per-run key | `embed-llama::LlamaServer` | first use → idle for `idle_shutdown_secs` (45 s) or `RunEvent::Exit`; starts again on the next request |
| `fm serve --socket` | child process, Unix socket | `llm-fm` | first generation → `RunEvent::Exit` |
| SQLite | WAL connection | `store-sqlite` | app |

- **`llama-server`** (ADR 0006): pidfile + key file in `<data>/run/`, log in `<data>/logs/llama-server.log`. A healthy server from a previous session, same binary and model, is reused; a stale one is replaced; processes that are not our binary never receive a signal. It runs with `--cache-ram 0 --no-cache-prompt --parallel 1`: the default prompt cache (up to 8 GiB) is of no use for embeddings and took the process from ~1 GB to ~10 GB after an import. After `idle_shutdown_secs` without requests (`embedding.json`, default 45, `0` = never), the process is stopped to give the memory back; while a `UseGuard` (`LlamaServer::begin_use`, held for the whole `embed_batch`) is alive, it does not stop.
- **`fm`** (ADR 0002): compatibility (macOS ≥ 27, native arm64, `fm` present) checked once; `status()` via `fm available` with a 30 s cache (exit 69 ⇒ `LicenseRequired`). Socket at `$TMPDIR/nlmx-fm-<pid>-<n>.sock` (≤ 103 bytes, else `/tmp`); restarted if it dies; one generation at a time. If `serve` does not start or drops before answering, it uses `fm respond --stream -i <instructions> <prompt>` and only tries `serve` again after 5 min.

## 9. Models

- **Runtime (part of the app):** llama.cpp's `llama-server`, build pinned in `scripts/bootstrap.sh`, shipped as a sidecar; detected by `LlamaCppRuntime` (`InferenceRuntime`). Never downloaded or removed.
- **Models (on demand):** `LocalModelProvider` (`models-catalog`) from the embedded catalog — upstream revision pinned in `version` and `url`, size and SHA-256 verified.
  - Storage: `<data>/models/<id>/<version>/<file>.gguf` + `manifest.json`; download in progress as `.part`.
  - **Mandatory confirmation:** `plan_download`/`plan_update` return a `DownloadPlan` (size, license, space); `download` only accepts a `ConfirmedDownload`, created by `DownloadPlan::confirm()` — called only by the `download_model` command (dialog button).
  - Download with resume via `Range`, incremental SHA-256, progress, cancellation, disk check (margin: the larger of 5% and 256 MB); a checksum mismatch deletes the `.part`.
  - Quick verification (manifest, size, `GGUF` magic) in the status; full SHA-256 in `verify`.
  - Update: downloads into a new folder, verifies, switches the active one and only then removes the old one.
  - Activation writes `<data>/embedding.json`, read by `LlamaCppEmbeddingProvider`. After activating/removing, `apply_model_change` stops the old `llama-server` and regenerates the vectors (all of them if the model changed; otherwise only the pending ones).

## 10. SQLite database

`~/Library/Application Support/dev.nlmx.desktop/nlmx.sqlite3`, migrations in `crates/adapters/store-sqlite/migrations/NNNN_name/{up,down}.sql` (all reversible, STRICT tables):

| Migration | Tables |
|---|---|
| 0001 | `app_settings` |
| 0002 | `documents`, `document_pages`, `document_chunks`, `document_chunks_fts` (FTS5, `unicode61 remove_diacritics 2`) |
| 0003 | `collections`, `collection_documents` |
| 0004 | `embedding_models`, `embedding_jobs` |
| 0005 · 0007 | `conversations`, `messages`, `citations`, `conversation_scopes` |
| 0006 | `chunk_embeddings` (chunk → model → vector, with the chunk's `content_hash`) |
| 0008 | `message_page_refs` |
| 0009 | `secure-delete` option of `document_chunks_fts` (ADR 0008) |
| 0010 | `conversations.mode` (free/documents) and `messages.grounding` (ADR 0009) |
| 0011 | `documents.format`/`mime_type`/`metadata`/`normalizer_version`/`previewable` (generated), `document_sections`, `chunk_provenance` — additive only (ADR 0013) |
| 0012 | `citations.document_type`/`document_name`/`locator` — source provenance for any format (ADR 0015) |

Vectors: one vec0 table `chunk_vectors_<embedding_model_id>` per vector space (model + revision + dimension), created at runtime by `create_index`; rowid = chunk id (ADR 0005).

## 11. Packaging

`.app`/`.dmg` aarch64, `minimumSystemVersion 27.0`: `llama-server` as a sidecar in `Contents/MacOS`, llama.cpp dylibs and `libpdfium` in `Contents/Frameworks`, SQLite and sqlite-vec statically linked in the binary, Tailwind compiled at build time, HTMX vendored. Models and `fm` stay outside the bundle. Details, signing and verification: [`RELEASE.md`](RELEASE.md).

## 12. Planned replacements

Each one is a new adapter, with no change in `application`:

| Component | Possible replacement |
|---|---|
| PDFium | MuPDF, pdf-rs, helper process (crash isolation) |
| `llama-server` | MLX, Core ML, candle; or `llama-server` on a Unix socket |
| sqlite-vec | usearch, LanceDB, custom HNSW |
| FTS5 | tantivy |
| `fm serve` | llama.cpp generator, opt-in remote provider |
| HTMX/Tailwind | another UI (only `ui-web`) |
| Tauri | another shell (only `src-tauri`) |

## Interface language

`crates/i18n` holds the Fluent catalogs (pt-BR, en, es), locale normalization and resolution (saved choice → macOS language → English) and locale-aware number/size formatting. `ui-web` resolves the saved/system language before the first request (`language.rs`, setting `ui.language` in `app_settings`), renders text through `nlmx_i18n::t*`, and switches live through `POST /settings/language`. The answer language of the model is independent (`ResponseLanguage::Auto`). See ADR 0022.
