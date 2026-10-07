# ADR 0011 — Parser layer

- Status: accepted
- Date: 2026-10-04
- Addresses: reading PDF, Markdown, TXT, CSV and EPUB with a common output (continues ADR 0010).

## Context
Ingestion only knew PDF: `DocumentEngine` → `StructureAnalyzer` → `Chunker`, with a mandatory page and boxes on every block. To index other formats without changing the RAG pipeline, each format must be read into a single intermediate representation that preserves structure, metadata and location until chunking.

## Decision
- **`DocumentParser` port** (`application::ports`): `document_type`, `version`, `supports_type`, `supports_mime` and `parse(&DocumentSource) → ParsedDocument`. A parser knows nothing about chunking, embeddings or RAG. `ParseError` (domain) has messages without content, file name or path.
- **`ParsedDocument`** (`domain::parsed`): metadata, `pages` (PDF only: size, `has_text`; input for the viewer and `NeedsOcr`), sections (`DocumentSection`: title, level, path, blocks) and warnings (`ParseWarning`, without content). Blocks (`ContentBlock`) have a structural kind (`ContentKind`: heading, paragraph, list item, code block, table, CSV record with column names), the readable text used for embeddings and a `SourceLocation`. Nothing becomes plain text before chunking.
- **`ParserRegistry`** (`application::services::parsing`) picks the parser by declared format or by extension; adding a format means registering a parser in the composition root.
- **`PdfDocumentParser`** lives in `application`: it composes `DocumentEngine` (PDFium) and `StructureAnalyzer` and reuses `read_layouts`, extracted from `DocumentIngestion` (adapters do not depend on each other). PDF extraction was not rewritten.
- **New adapters:** `parser-text` (`MarkdownDocumentParser`, `TextDocumentParser`, `CsvDocumentParser`; UTF-8/UTF-16 decoding with BOM and Windows-1252 fallback) and `parser-epub` (zip + XML; refuses DRM; zip-bomb limits). All have a pure synchronous function and run in `spawn_blocking`.
- **CSV:** one `Record` per data row (detected header or "coluna N" columns), with the row number; the chunker will group rows repeating the header.
- Every parser passes the same contract suite (`nlmx_testing::document_parser_contract`).

## Consequences
- `DocumentIngestion`, `wiring.rs`, the chunker, the database and the UI **do not change in this step**: the parsers exist, but ingestion still uses `DocumentEngine` directly. The next step connects `ParserRegistry` to chunking and persistence (location column, migration).
- `tests/tests/architecture.rs` now expects 11 adapters.
- A paragraph without line breaks in TXT becomes a single block; the chunker splits it and the chunk location inherits the block's range.

## Update: semantic CSV
- **Readable record:** each data row becomes a `Record` block whose text is `Registro N:` (Record N:) followed by one `Coluna: valor` (Column: value) line per non-empty cell (spaces and inner line breaks collapsed). `fields` holds all columns, including empty ones. `N` is the data row (the header does not count), equal to the location `SourceLocation::Csv { row_start, row_end }`, which is the requested `Rows`. The label is "linhas N–M" (rows N–M); in a spreadsheet the header is row 1, so the spreadsheet row is `N + 1` when there is a header.
- **Verified header:** it is a header when the first row has no number, date or boolean, has distinct names and there are rows below it. It is *strong* when there is a type contrast with the rows below; in an all-text file it is only accepted if the cells are short and unique, and then the `UncertainHeader` warning is emitted (the heuristic is inherently ambiguous in this case). Without a header, columns are named "coluna N" (`NoHeaderRow`). A single row is data.
- **Dataset:** `DocumentMetadata.dataset` (`DatasetMetadata`: columns with type — text, number, date, boolean —, delimiter, whether there is a header and the number of rows when the file was read in full). Delimiters `, ; tab |`; UTF-8 encoding (or UTF-16 with BOM) with Windows-1252 fallback.
- **Incremental reading:** `CsvStream` (`parser-text`) validates the encoding in blocks, reads a sample from the start to detect delimiter, header and types, and delivers records one at a time, with memory proportional to one record. `DocumentParser::parse` uses the same stream but collects up to 250 thousand rows (`TooLarge` above); the stream goes up to 5 million rows and 1 GiB (`CsvLimits`).
- **`RecordChunker`** (`chunker-structural`, lazy): groups records up to `target_tokens` with a preamble (`Arquivo`, `Colunas`, `Linhas a–b`; File, Columns, Rows a–b) in every chunk, with no overlap (`overlap_tokens` is ignored: records are atomic), merges a tail smaller than `min_tokens`, truncates a long column list and splits by fields a record larger than the budget ("Registro 7 (parte 2/3)"; Record 7 (part 2/3)). The chunk is a `DocumentChunk` with `Csv { a, b }`, `file_name` and `columns`.
- The chunker is not yet a port: nothing calls it. The port and per-format chunker selection arrive with the step that reconnects `DocumentIngestion` to all formats (migration, `fs-library`, `wiring.rs`).

## Addendum: TSV

`.tsv` (`text/tab-separated-values`) is the same `DocumentType::Csv`: same parser extension, `RecordChunker` and `SourceLocation::Csv`. The only difference is that the `.tsv` extension forces the `\t` delimiter (`CsvStream::open_with_delimiter`) instead of detecting it. There is no type, migration or viewer of its own.
