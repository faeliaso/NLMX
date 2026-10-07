# ADR 0004 — Single SQLite with FTS5 and sqlite-vec

- Status: accepted (RRF fusion superseded by ADR 0007)
- Date: 2026-10-02

## Context
We need metadata, text, a lexical index and a vector index that are local, consistent with each other and free of external services.

## Decision
- One SQLite file (WAL) per library; SQLite `bundled` and sqlite-vec statically linked.
- FTS5 with external content (`chunks`), tokenizer `unicode61 remove_diacritics 2`, BM25.
- sqlite-vec (`vec0`) for KNN; fusion with BM25 via RRF in the `Retriever` (application layer).
- One writer connection (actor) + read pool; `commit_document` writes chunks, FTS and vectors in a single transaction.
- `LexicalIndex` and `VectorIndex` are separate ports, even when implemented by the same crate.

## Consequences
- Atomic ingestion per document; backup = one directory.
- sqlite-vec is pre-1.0 and brute-force: fine for the MVP (~100k chunks); larger scale may require quantization or replacing the `VectorIndex` adapter.
- FTS5 has no Portuguese stemming; the vector covers semantics.

## Update (2026-10-03)
- RRF fusion was replaced by weighted fusion of normalized scores (ADR 0007).
- The vector port is named `VectorStore` (not `VectorIndex`). There is no single `commit_document`: `save_extraction` writes pages, chunks and FTS in one transaction, and each batch of vectors is written (vec0 + `chunk_embeddings`) in another, afterwards.
