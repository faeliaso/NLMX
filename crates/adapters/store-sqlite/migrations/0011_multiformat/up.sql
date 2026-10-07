-- Documents of more formats than PDF (ADR 0013). Additive only: no table is recreated and no
-- existing row is rewritten (an UPDATE would fire `documents_touch` and bump every PDF's
-- version/updated_at). `document_chunks` — the FTS5 content table, whose triggers and the
-- runtime `chunk_vectors_<id>_chunk_delete` triggers hang on it — is left exactly as it is.

ALTER TABLE documents ADD COLUMN format TEXT NOT NULL DEFAULT 'pdf'
    CHECK (format IN ('pdf', 'markdown', 'text', 'csv', 'epub'));
-- NULL on rows from before this migration: read as the media type of `format`.
ALTER TABLE documents ADD COLUMN mime_type TEXT;
-- DocumentMetadata as JSON (author, language, publisher, dataset columns, ...).
ALTER TABLE documents ADD COLUMN metadata TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata));
ALTER TABLE documents ADD COLUMN normalizer_version INTEGER;
-- Only a PDF can be opened in the viewer; derived, so it can never disagree with `format`.
ALTER TABLE documents ADD COLUMN previewable INTEGER GENERATED ALWAYS AS (format = 'pdf') VIRTUAL;

CREATE INDEX documents_format ON documents (format);

-- The structure of a document (headings, chapters, the text before the first heading). Text
-- lives in the chunks only, so nothing is stored twice and removal has one place to clean.
CREATE TABLE document_sections (
    id          INTEGER PRIMARY KEY,
    document_id INTEGER NOT NULL REFERENCES documents (id) ON DELETE CASCADE,
    parent_id   INTEGER REFERENCES document_sections (id) ON DELETE CASCADE,
    kind        TEXT NOT NULL CHECK (kind IN ('heading', 'body')),
    title       TEXT,
    level       INTEGER NOT NULL DEFAULT 0 CHECK (level >= 0),
    ordinal     INTEGER NOT NULL CHECK (ordinal >= 0),
    -- JSON array of the headings that enclose the section, its own last: ["Capítulo 3", "Modelos"].
    path        TEXT NOT NULL CHECK (json_valid(path)),
    -- JSON SourceLocation where the section starts.
    locator     TEXT NOT NULL CHECK (json_valid(locator)),
    metadata    TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata)),
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (document_id, ordinal),
    CHECK (kind <> 'heading' OR title IS NOT NULL)
) STRICT;

CREATE INDEX document_sections_parent ON document_sections (parent_id);

-- Provenance of the chunks of every format but PDF. A PDF chunk keeps its page, page_end and
-- bboxes in `document_chunks` (the source of truth, read by the viewer and the citations);
-- a chunk of another format stores page_start = page_end = 1 there — a placeholder the
-- schema's NOT NULL CHECK (page_start >= 1) forces, meaningless unless documents.format = 'pdf' —
-- and its real location here.
CREATE TABLE chunk_provenance (
    chunk_id   INTEGER PRIMARY KEY REFERENCES document_chunks (id) ON DELETE CASCADE,
    section_id INTEGER REFERENCES document_sections (id) ON DELETE SET NULL,
    -- JSON SourceLocation: lines, offsets, rows, chapter...
    locator    TEXT NOT NULL CHECK (json_valid(locator)),
    -- JSON ChunkMetadata: file name, title, language, columns.
    metadata   TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(metadata)),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE INDEX chunk_provenance_section ON chunk_provenance (section_id);
