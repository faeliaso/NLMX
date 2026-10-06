# ADR 0014 — Multi-format indexing pipeline

- Status: accepted
- Date: 2026-10-04
- Addresses: importing PDF, Markdown, TXT, CSV and EPUB end to end (continues ADRs 0010–0013).

## Context
Parsers (ADR 0011), the content pipeline (ADR 0012) and multi-format persistence (ADR 0013) existed, but production ingestion only read PDF: `import` hard-coded `DocumentType::Pdf`, the library stored `<sha>.pdf` and the dialog only accepted PDF.

## Decision
- **A single path, chosen by the registry.** `DocumentIngestion` receives a `ContentPipeline` (`parse → normalize → chunk`); the format comes from `DocumentType::from_path` and the parser from `ParserRegistry`. No `if pdf/markdown/csv` outside the parsers. With no parser for the extension, the import fails before copying the file. Without a `ContentPipeline` (old tests with fakes) ingestion stays on the legacy PDF-only path.
- **Reused states**, without a migration: `queued → extracting (parse) → structuring (normalization) → chunking → embedding → indexed`, plus `failed` and `needs_ocr`. The screen shows a finer phase (`IngestPhase`: reading, structure, chunks, saving, embeddings, waiting for model, indexed, failed).
- **Progress through a port.** `ProgressSink` (`application::ports`) receives `IngestProgress` (document, file name, format, phase, fraction 0–1, number of chunks, status). Ingestion reports up to saving (60%); `EmbedDocuments` reports per batch (60–100%). The Tauri adapter (`ProgressRelay`) emits the `ingest-progress` event and keeps what is in progress for `ingest_progress`. The file name goes only to the UI, never to logs.
- **Isolated errors.** Each file is imported in its own task; a parser failure becomes `Failed` on the document (`error` with the cause, without content or path) and the `IngestFailed` measurement; a panic only fails that file. `import_many` exposes the same behaviour to callers without Tauri.
- **Idempotent reindexing.** `reindex(id)` reruns everything, whatever the status: `save_processed` replaces pages, chunks and structure in one transaction (the FTS follows through triggers) and `EmbedDocuments` deletes the document's vectors before inserting. Document ids are kept.
- **Changed file = same document.** `find_by_original_path` + `replace_source`: if the original path is already known and the hash changed, the document points to the new copy, returns to `queued` and is reindexed; the old copy leaves the library and the viewer cache is discarded. Citations already stored keep the cited text (`citations.chunk_id` becomes `NULL` when the chunks are replaced); for PDF, the highlight in the viewer starts using the new file. The same hash at another path is still a duplicate.
- **Multi-format library.** `fs-library` stores `<sha>.<file ext>` (`.bin` when there is no usable extension), reuses an existing copy of the same hash with another extension, and `remove`/`prune` recognize any extension (old `<sha>.pdf` files remain valid).
- **Desktop.** `wiring.rs` builds the registry (PDF, Markdown, TXT, CSV, EPUB), `TextNormalizer` and `MultiFormatChunker`; the file picker uses `ParserRegistry::supported_types()`.

## Background import
`import_documents` only opens the picker and **enqueues** the paths; it responds at once (the button does not stay in "loading") and more can be imported while others are processing. **Registration** (`DocumentIngestion::enqueue`: the document starts to exist as `queued` and already appears in the list) happens immediately, for all files and in parallel with what is already processing — a second batch imported while the first is still running also appears at once. Only **processing** (`process`, the pipeline) is serialized in a worker (`src-tauri/src/importer.rs`), one file at a time, each in its own task (a panic only fails that file). Importing again a file that is queued or being read is a duplicate (only a `failed` document is reprocessed; one interrupted by closing the app comes back through `resume` at startup). The list shows the real status of each stage ("Na fila" (Queued), "Lendo o arquivo" (Reading the file), "Estruturando" (Structuring), "Dividindo em trechos" (Splitting into chunks), "Aguardando embeddings" (Waiting for embeddings), "Indexado" (Indexed), "Falhou" (Failed)). `import` is still `enqueue` + `process`. The app emits `documents-changed` (on every registered document and every stage change; the UI reloads the list with debounce) and one `import-finished` per batch, which becomes the summary toast. While a batch is running, `IndexingActivity` is active.

## Consequences
- The legacy path (`engine`/`analyzer`/`chunker` in `DocumentIngestion`, `save_extraction`) remains only for tests with fakes and may be removed in a future cleanup.
- If PDFium fails to load, all ingestion remains unavailable (as before), not just PDF.
- If the new version of a changed file fails to process, the chunks of the previous version remain until a new `save_processed`; the document is `failed` and can be reprocessed.
- There is no cancelling or pausing of an import in progress.
- Since 2026-10-04 the Indexação screen no longer shows live progress (the "Processando agora" (Processing now) panel, the "Em andamento" (In progress) list and the coverage bar were removed); `ProgressSink`, the `ingest-progress` event and the `ingest_progress` command remain, with no consumer in the UI.
