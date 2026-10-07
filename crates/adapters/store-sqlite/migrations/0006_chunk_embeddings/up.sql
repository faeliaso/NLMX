-- Which chunk has a vector in which embedding model: the link in
-- documents → document_chunks → chunk_embeddings → chunk_vectors_<embedding_model_id> (rowid = chunk_id).
CREATE TABLE chunk_embeddings (
    chunk_id           INTEGER NOT NULL REFERENCES document_chunks (id) ON DELETE CASCADE,
    embedding_model_id INTEGER NOT NULL REFERENCES embedding_models (id) ON DELETE CASCADE,
    -- Denormalized from the chunk for per-document filters and deletes.
    document_id        INTEGER NOT NULL REFERENCES documents (id) ON DELETE CASCADE,
    -- The chunk's content_hash when it was embedded: a different current hash means a stale vector.
    content_hash       TEXT NOT NULL,
    embedded_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (chunk_id, embedding_model_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX chunk_embeddings_model_document ON chunk_embeddings (embedding_model_id, document_id);
