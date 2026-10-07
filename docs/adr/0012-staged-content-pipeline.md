# ADR 0012 — Staged content pipeline

- Status: accepted
- Date: 2026-10-04
- Addresses: normalization and chunking for PDF, Markdown, TXT, CSV and EPUB (continues ADRs 0010 and 0011).

## Context
PDF chunking lived in `chunker-structural` (page and boxes), CSV chunking in a standalone chunker, and each parser cleaned text in its own way. To index several formats without duplicating logic in the parsers, each stage must have a single responsibility and provenance (page, lines, offsets, chapter, heading path) must reach the chunk intact.

## Decision
- **Explicit stages, each behind its own port:** the *parser* extracts and structures (`DocumentParser` → `ParsedDocument`), the *normalizer* cleans text (`DocumentNormalizer`), the *chunker* cuts (`DocumentChunker` → `DocumentChunk`) and *embedding* generates vectors (`EmbedDocuments`, after the chunks are stored). `application::services::pipeline::ContentPipeline` composes the first three (`parse → normalize → chunk`) and returns `ProcessedDocument` (metadata, pages, chunks, warnings and the version of each stage). It uses nothing from embeddings (`architecture.rs` checks this).
- **Complete chunk** (`domain::parsed::DocumentChunk`): `document_id`, `chunk_id` (`None` until stored), `index` (position), text, `token_count` (when a counter is available), `section_path` ("Capítulo 3" › "Embeddings" › …), `location: SourceLocation`, `metadata` (`ChunkMetadata`: type, title, file name, language, columns) and content hash. The section path lives in the chunk, not in the text: the embeddings stage prepends it.
- **Normalizer** (`normalizer-text`): NFC; removes BOM, zero-width characters, soft hyphen and control characters; Unicode spaces and NBSP become a space; ligatures are expanded; whitespace is collapsed by block kind (code keeps spaces and line breaks; table and record keep line breaks). It is pure, idempotent and **never changes block kind or location**; it only drops a block that becomes empty. PDF-specific cleanup (hyphenation, header and footer) stays in `structure-heuristic`, because it depends on spans and coordinates.
- **Single chunker:** a generic core in `chunker-structural` works on the blocks of a `ParsedDocument` and computes the chunk location with `SourceLocation::merge` (union of the source blocks) and the overlap location with `trailing()`. `MultiFormatChunker` (`DocumentChunker`) uses the core for all formats and `RecordChunker` for CSV. The PDF `StructuralChunker` (`Chunker`, in production) became a wrapper around the same core with output identical to the previous one (equivalence test against the old algorithm and against real PDFs); version `1` was kept. Outside PDF, the only changes are splitting code and tables by lines (never by sentences) and joining list items with `\n`.
- **Location per format:** PDF → pages and boxes; Markdown → heading path and lines; TXT → range `[start, end)` in characters of the decoded text; CSV → data rows; EPUB → chapter and section. A large block split into several chunks **inherits the block's range** (no refinement inside the block); for this reason the TXT parser splits paragraphs longer than ~2,000 characters at single line breaks.
- Reusable contracts in `nlmx-testing`: `document_normalizer_contract` and `document_chunker_contract` (index, `document_id`, valid location of the document's type, section path, coverage of each block by a chunk and no word lost).

## Consequences
- None of this is wired to `DocumentIngestion`, the database or the UI yet: ingestion still uses `DocumentEngine` → `StructureAnalyzer` → `StructuralChunker`. ADR 0014 reconnected ingestion to `ContentPipeline` (migration with optional location and `locator` column, `fs-library` without a hard-coded `.pdf`, `wiring.rs`, file picker).
- Applying the normalizer to already indexed PDFs would change the `content_hash` of existing chunks; for this reason the normalizer has a `version` and only enters ingestion together with reindexing.
- `tests/tests/architecture.rs` now expects 12 adapters.
