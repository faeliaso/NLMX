# ADR 0013 — Multi-format persistence

- Status: accepted
- Date: 2026-10-04
- Addresses: storing documents, structure, chunks and provenance of PDF, Markdown, TXT, CSV and EPUB (continues ADRs 0010 to 0012).

## Context
The database only knew PDF: `document_chunks.page_start NOT NULL CHECK (>= 1)`, `bboxes`, `documents.pdf_created_at`, no format, no structure. Relaxing the `NOT NULL` would require recreating `document_chunks`, which is the FTS5 content table and the table the FTS triggers hang on, as do the `chunk_vectors_<id>_chunk_delete` triggers, created at runtime, that delete the vec0 rows (vec0 has no cascade). Recreating it would drop those triggers and risk the rowids that link chunks to vectors.

## Decision
- **Migration 0011, additive only** (`up`: no `UPDATE`, no table recreation; `down` tested):
  - `documents` gains `format` (default `'pdf'`, with `CHECK`), `mime_type` (NULL in old rows = the MIME of `format`, read by `DocumentRecord`), `metadata` (JSON of `DocumentMetadata`), `normalizer_version` and `previewable`, a **generated** column (`format = 'pdf'`), which never diverges from the format. No existing row is rewritten: the `documents_touch` trigger does not fire, so `version` and `updated_at` of PDFs do not change.
  - `document_sections`: structure only (`kind` heading/body, title, level, `ordinal`, `path` in JSON, `locator` in JSON, `parent_id`). The text stays in the chunks, with no copy.
  - `chunk_provenance` (key `chunk_id`, `section_id`, `locator`, `metadata`): the location (`SourceLocation` in JSON) and the metadata of each chunk stored with `save_processed`.
  - `document_chunks`, FTS, vectors, `chunk_embeddings`, citations and `message_page_refs` **do not change**.
- **Legacy page columns.** A PDF chunk keeps `page_start`, `page_end` and `bboxes` in `document_chunks` (the viewer and citations read from there); the PDF `locator` stores only the page range. A chunk of another format stores `page_start = page_end = 1` — a sentinel value required by `NOT NULL CHECK (>= 1)`, **meaningless** outside `documents.format = 'pdf'` — and `bboxes = '[]'`; the real location is in `chunk_provenance`. Chunks of PDFs indexed before this migration have no row in `chunk_provenance`: they are read as `SourceLocation::Pdf` from the page columns.
- **Conceptual model → project schema:** name = `title`/`original_filename`; path = `original_path`/`library_path`; size = `file_size`; hash = `sha256`; chunk = `document_chunks` (`text`, `ordinal`, `token_count`); the chunk's `section_id` and `metadata` in `chunk_provenance`; embedding = `chunk_embeddings` + `embedding_models` (model, dimensions) + the model's vec0 table. It follows the project convention: `kind` (not `type`) and `ordinal` (not `position`).
- **Ports:** `NewDocument.document_type`, `DocumentRecord.document_type`/`mime_type`, and in `DocumentRepository` the methods `save_processed(StoredExtraction)`, `chunks_of` and `sections_of`. `save_extraction` (PDF, in production) stays the same. `save_processed` runs in one transaction, refuses a chunk of a format other than the document's without writing anything, and replaces everything derived from the file, so saving twice gives the same rows.
- **Idempotency:** `documents.sha256` is `UNIQUE` and `insert` uses `ON CONFLICT DO NOTHING`: the same content is the same document, whatever the name or extension, and the first imported format wins. Reindexing deletes the old chunks; the FTS, the vec0 rows (runtime triggers) and `chunk_embeddings` (cascade) follow.
- **Page search:** the page filter only applies to documents with `format = 'pdf'` (`lexical.rs`); the sentinel page does not match.
- **`down.sql`** deletes non-PDF documents before dropping the columns (the previous version cannot read them; the cascade cleans chunks, FTS, vectors and structure). PDFs are left intact.

## Consequences
- Nothing stores non-PDF documents yet: ingestion, RAG, citations and the UI remain PDF-only. `chunk_provenance` and `document_sections` await the step that reconnects `DocumentIngestion` to `ContentPipeline` (`ProcessedDocument::into_stored`).
- Citations of formats without pages need their own column (`citations.page_number` is `NOT NULL CHECK (>= 1)`): a migration in the RAG step.
- Every new table that holds data derived from a document depends on `documents (id) ON DELETE CASCADE`, so `RemoveDocument` covers it; the tests check tables, FTS, vectors and the database bytes after removal.
- Integrity query used in the tests: no chunk of a format other than pdf without `chunk_provenance`; the page of a non-PDF chunk is the sentinel; the `kind` of the `locator` is the document's `format`; every vec0 row has a chunk and a row in `chunk_embeddings`, and vice versa.
