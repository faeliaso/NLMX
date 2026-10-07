# ADR 0018 — Notes: pasted text as a source, without a file

- Status: accepted
- Date: 2026-10-05
- Addresses: RF01 (new ways to add knowledge). Extends ADRs 0011–0017; does not supersede any.

## Context
All knowledge entered through a file (`<sha>.<ext>` in the library). The user also wants to paste copied text and use it as a source, with the same treatment as any document: normalization, chunking, embeddings, SQLite, sqlite-vec and RAG.

## Decision
- **One `DocumentType::Note`** (`note`, "Nota" (Note)), never `previewable` nor `is_paged`. There is no note viewer, editor or preview: it opens the **Informações da fonte** (Source information) panel (ADR 0016), without "Arquivo" (File) or "Tamanho" (Size). The extension `nlmx-note` and the MIME `application/x-nlmx-note` are reserved only to keep lookups total; they are never offered in the file picker.
- **No parallel pipeline.** `DocumentIngestion::enqueue_note` registers the document (`queued`) and the rest is the usual `ContentPipeline` (`parse → normalize → chunk`, then `EmbedDocuments`). A note uses the same queue (`ImportQueue`), the same events (`documents-changed`, `import-finished`), the same `IndexingActivity`, the same `RemoveDocument`.
- **No physical file: the text lives in SQLite.** Migration `0014_note`: swap of the `documents.format` column (CHECK with `'note'`, like 0013, never recreating `documents`) and `documents.note_text TEXT`. `library_path` stays empty and `original_path` is `NULL`. Reindexing, retrying and resuming indexing at startup read the text from the database: `DocumentSource` gained `text: Option<Arc<str>>` (`DocumentSource::note`) and the parser never reads a path. Removal already covers the column (cascade + `secure_delete`); `FileStore::remove` of a nonexistent file is not an error.
- **Parser.** `NoteDocumentParser` (`parser-text`) reuses `TextDocumentParser::parse_bytes` (paragraphs by blank lines, long paragraph split) and rewrites the locations to `SourceLocation::Note { start, end }`: character range of the note text, like `Text`. `Text` does not fit because `save_processed` requires the location to match the document's format. No page, sheet or file is invented; a source's label is `<title> · caracteres a–b` (characters a–b).
- **Text and title (`domain::note`).** `clean_note_text` unifies `\n`, strips trailing spaces on lines and blank lines at the ends, and repeats them at most once; it refuses empty/whitespace-only text (`NoteError::Empty`) and more than 1 million characters. The title is the first line when it looks like a title (short, without sentence-ending punctuation; `#` is removed), otherwise the start of the content; it goes in `documents.title`/`original_filename` and does not pass through `title_from_filename`. There is no title field in the modal.
- **Duplicate.** A note's "hash" is the SHA-256 of the cleaned text with a domain-separation prefix (`FileStore::digest_text`), so the same note is a `Duplicate` and a note never collides with a file of the same bytes.
- **Interface.** The "Adicionar nota" (Add note) button sits next to "Importar" (Import) and opens a `<dialog>` with a `textarea`, Cancelar (Cancel) and Inserir (Insert) (disabled without non-blank text; single submit). Submission is `POST /documents/notes` (HTMX), which calls the `NoteSubmitter` port (implemented in `src-tauri` over the `ImportQueue`); the result is a toast and the note appears in Documentos as `queued → … → indexed`.

## Consequences
- No existing format changes behaviour; every exhaustive `match` over `DocumentType`/`SourceLocation` gained the note arm.
- Editing a note does not exist (the hash would change): paste another one.
- The repository contracts (`document_store_contract`) now cover all formats, including `note`.
