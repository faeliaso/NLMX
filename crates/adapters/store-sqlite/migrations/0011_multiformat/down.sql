-- Downgrading drops every document that is not a PDF (the previous schema cannot read them:
-- their chunks carry the page 1 placeholder); the cascade removes their chunks, FTS rows,
-- embeddings and sections. PDFs are untouched.
DELETE FROM documents WHERE format <> 'pdf';
DROP TABLE chunk_provenance;
DROP TABLE document_sections;
DROP INDEX documents_format;
ALTER TABLE documents DROP COLUMN previewable;
ALTER TABLE documents DROP COLUMN normalizer_version;
ALTER TABLE documents DROP COLUMN metadata;
ALTER TABLE documents DROP COLUMN mime_type;
ALTER TABLE documents DROP COLUMN format;
