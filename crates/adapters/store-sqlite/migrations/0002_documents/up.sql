CREATE TABLE documents (
    id                INTEGER PRIMARY KEY,
    sha256            TEXT NOT NULL UNIQUE CHECK (length(sha256) = 64),
    title             TEXT,
    author            TEXT,
    language          TEXT,
    original_filename TEXT NOT NULL,
    original_path     TEXT,
    library_path      TEXT NOT NULL,
    file_size         INTEGER NOT NULL CHECK (file_size >= 0),
    page_count        INTEGER CHECK (page_count >= 0),
    has_text_layer    INTEGER CHECK (has_text_layer IN (0, 1)),
    status            TEXT NOT NULL DEFAULT 'queued' CHECK (status IN (
                          'queued', 'extracting', 'structuring', 'chunking', 'embedding',
                          'indexed', 'needs_ocr', 'failed')),
    error             TEXT,
    extractor_version INTEGER,
    chunker_version   INTEGER,
    pdf_created_at    TEXT,
    imported_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    indexed_at        TEXT,
    created_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    version           INTEGER NOT NULL DEFAULT 1,
    CHECK (status <> 'failed' OR error IS NOT NULL)
) STRICT;

CREATE INDEX documents_status ON documents (status);
CREATE INDEX documents_imported_at ON documents (imported_at DESC);

CREATE TRIGGER documents_touch AFTER UPDATE ON documents
FOR EACH ROW WHEN NEW.version = OLD.version
BEGIN
    UPDATE documents SET version = OLD.version + 1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id;
END;

CREATE TABLE document_pages (
    id          INTEGER PRIMARY KEY,
    document_id INTEGER NOT NULL REFERENCES documents (id) ON DELETE CASCADE,
    page_number INTEGER NOT NULL CHECK (page_number >= 1),
    width       REAL CHECK (width > 0),
    height      REAL CHECK (height > 0),
    char_count  INTEGER NOT NULL DEFAULT 0 CHECK (char_count >= 0),
    has_text    INTEGER NOT NULL DEFAULT 0 CHECK (has_text IN (0, 1)),
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (document_id, page_number)
) STRICT;

CREATE TABLE document_chunks (
    id           INTEGER PRIMARY KEY,
    document_id  INTEGER NOT NULL REFERENCES documents (id) ON DELETE CASCADE,
    ordinal      INTEGER NOT NULL CHECK (ordinal >= 0),
    text         TEXT NOT NULL,
    token_count  INTEGER NOT NULL CHECK (token_count >= 0),
    page_start   INTEGER NOT NULL CHECK (page_start >= 1),
    page_end     INTEGER NOT NULL,
    section_path TEXT,
    bboxes       TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(bboxes)),
    content_hash TEXT NOT NULL,
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (document_id, ordinal),
    CHECK (page_end >= page_start)
) STRICT;

CREATE INDEX document_chunks_pages ON document_chunks (document_id, page_start);

-- Lexical index (BM25) kept in sync with document_chunks. Search API comes later.
CREATE VIRTUAL TABLE document_chunks_fts USING fts5 (
    text,
    content = 'document_chunks',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER document_chunks_fts_insert AFTER INSERT ON document_chunks BEGIN
    INSERT INTO document_chunks_fts (rowid, text) VALUES (NEW.id, NEW.text);
END;

CREATE TRIGGER document_chunks_fts_delete AFTER DELETE ON document_chunks BEGIN
    INSERT INTO document_chunks_fts (document_chunks_fts, rowid, text) VALUES ('delete', OLD.id, OLD.text);
END;

CREATE TRIGGER document_chunks_fts_update AFTER UPDATE OF text ON document_chunks BEGIN
    INSERT INTO document_chunks_fts (document_chunks_fts, rowid, text) VALUES ('delete', OLD.id, OLD.text);
    INSERT INTO document_chunks_fts (rowid, text) VALUES (NEW.id, NEW.text);
END;
