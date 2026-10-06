# ADR 0005 — One vector space (vec0 table) per embedding model

- Status: accepted
- Date: 2026-10-02

## Context
Vectors from different models are not comparable, and `vec0` has a fixed dimension. The user may switch embedding models.

## Decision
- Entity `EmbeddingSpace{model_id, revision, dims}`; each has its own `chunks_vec_<space_id>` table.
- Exactly one active space. Switching models creates the new space and `reembed` jobs (reusing the chunks, without re-extracting PDFs); search stays on the old space until completion; activation is atomic and the old space is removed afterwards.
- Every vector is stored together with its space; queries always specify the space.

## Consequences
- Mixing vectors from different models is impossible.
- Model switching without search downtime, at the cost of temporary disk space.

## Update (2026-10-03)
- Actual table: `chunk_vectors_<embedding_model_id>` (vec0), created at runtime by `VectorStore::create_index` from `EmbeddingSpace::from_identity`; the chunk → model → vector link lives in `chunk_embeddings` (migration 0006).
- Model switching implemented more simply: `apply_model_change` stops the old `llama-server`, marks indexed documents as `embedding` and regenerates all vectors in the new space. During regeneration, search uses the new (partial) space and the lexical index; there is no atomic switch nor automatic cleanup of the old space.
