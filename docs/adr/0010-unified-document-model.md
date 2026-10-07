# ADR 0010 — Unified document model

- Status: accepted
- Date: 2026-10-04
- Addresses: evolution towards PDF, Markdown, TXT, CSV and EPUB (first step: domain only).

## Context
Today the system only knows PDF: there is no type for the document format, and the provenance of a chunk is always `page_start`/`page_end` plus boxes in PDF points (`ChunkDraft`, `ChunkView`, `MessageSource`, tables `document_chunks` and `citations`). To index and answer from other formats, the RAG must be able to cite a chunk without knowing how the document was read, and only PDF has a viewer.

## Decision
- New types in `nlmx-domain`, additive (no existing type changes fields or behaviour):
  - `document_type::DocumentType` (`Pdf`, `Markdown`, `Text`, `Csv`, `Epub`): stable name (`as_str`, same as serde), `from_extension`/`extensions` as the single source of accepted extensions, `previewable()` and `is_paged()`.
  - `source::SourceLocation`: enum with one variant per format (`Pdf` page + optional boxes; `Markdown` heading path + optional lines; `Text` range `[start, end)` in characters of the decoded text; `Csv` 1-based data rows excluding the header; `Epub` 1-based chapter + optional title/section). Validated constructors and `validate()` for deserialized values; `label()` in Portuguese for citations.
  - `source::SourceReference`: document, title, chunk, `SourceLocation` and section path. The type and the preview **derive from the location**, so a type and a location can never disagree.
  - `parsed::{Document, DocumentMetadata, DocumentSection, ParsedDocument, DocumentChunk}`: the neutral output of any parser (the typed block structure is detailed in ADR 0011). `ParsedDocument::new` requires every section and block to be of the declared format. `DocumentChunk::from_draft` reads a PDF `ChunkDraft` as a neutral chunk (one-way view).
- **`previewable` does not decide indexing.** Every `DocumentType` can be indexed and used by the RAG; `previewable` (PDF only) just says whether a viewer exists. There is no `is_indexable`.
- The domain now depends on `serde` (derive) for `DocumentType`, `SourceLocation` and the types above, and on `serde_json` only in tests. `BoundingBox` and `PageBox` gain `Serialize`/`Deserialize`. It still depends on no workspace or infrastructure crates (`tests/tests/architecture.rs`).
- `document::DocumentMetadata` (PDF, with `pdf_version` and `page_count`) stays as it is; `parsed::DocumentMetadata` is the neutral type, in another module.

## Consequences
- Nothing uses the new types yet: ports, database, RAG and UI remain PDF-only. The next steps (a `locator` column holding the `SourceLocation` as JSON, an extraction port, neutral citations) will consume them.
- Citations of formats without a viewer are not clickable: they show the location label.
- Changing the JSON format of `SourceLocation` after it is stored in the database requires a migration; the format is pinned by tests.
