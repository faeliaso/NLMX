# ADR 0008 — Document removal, with the history that used it

- Status: accepted
- Date: 2026-10-03
- Addresses: RF05 (remove a document and all derived data).

## Context
The database cascades (migrations 0002–0008) already deleted pages, chunks, FTS5, vectors, jobs, collections, citations, scopes and page references when a document was deleted. That was not enough:
- questions and answers stayed in the history, and answers often transcribe the document;
- a conversation scoped to the document silently lost its scope and started searching everything;
- the `<data>/library/<sha>.pdf` copy stayed on disk;
- a `DELETE` does not erase bytes: the text stayed in SQLite free pages, in the WAL and in FTS5 segments.

## Decision
- `DocumentRepository::remove` does everything in one transaction:
  - deletes the conversations scoped to the document (`conversation_scopes`);
  - in the others, removes each question + answer pair whose answer received any passage from it (`citations`, cited or not) or has a `[página N]` from it (`message_page_refs`). The question is the `user` message immediately before;
  - deletes the conversations left without messages;
  - deletes the document, and the cascades take care of the rest.
- Physical trace: `PRAGMA secure_delete = ON` on every connection, FTS5's `secure-delete` option (migration 0009) and `wal_checkpoint(TRUNCATE)` after the removal. A test checks that removed text does not appear in the bytes of the database or the WAL.
- `RemoveDocument` refuses the removal while the document is being read (`queued`/`extracting`/`structuring`/`chunking`), deletes the library copy and clears the viewer's text cache. The user's original file is never touched.
- The database is the source of truth: if the library file cannot be deleted, the removal stands anyway, and `RemoveDocument::prune_library` (at startup) deletes files with no document. Files modified less than 10 min ago are spared, because of an import in progress.
- Ids can be reused (`documents.id` is not `AUTOINCREMENT`): page image URLs carry `v=<SHA-256 prefix>`, and the spans cache is cleared on removal.
- The UI asks for confirmation and shows what will be deleted: chunks, conversations and question-and-answer pairs.

## Consequences
- Removal is final; there is no undo.
- A conversation with several documents loses only the exchanges that used the removed one. Later answers remain, even if the follow-up rewrite used the removed pair as context.
- `secure_delete` zeroes freed pages, a small write cost for the app's volumes.
- Outside the app's reach: backups (Time Machine) and the original file.
- sqlite-vec vectors are not text; vec0 may keep bytes of removed vectors in its blocks until it reuses them.
