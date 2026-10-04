-- Removing a chunk erases its terms from the FTS5 index right away instead of leaving them in
-- the index segments until the next merge (a removed document must leave no searchable trace).
INSERT INTO document_chunks_fts (document_chunks_fts, rank) VALUES ('secure-delete', 1);
