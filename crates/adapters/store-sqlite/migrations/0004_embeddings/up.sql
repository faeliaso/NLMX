CREATE TABLE embedding_models (
    id              INTEGER PRIMARY KEY,
    model_key       TEXT NOT NULL CHECK (length(model_key) > 0),
    revision        TEXT NOT NULL,
    display_name    TEXT NOT NULL,
    dims            INTEGER NOT NULL CHECK (dims > 0),
    max_tokens      INTEGER NOT NULL CHECK (max_tokens > 0),
    file_path       TEXT,
    file_sha256     TEXT CHECK (file_sha256 IS NULL OR length(file_sha256) = 64),
    file_size       INTEGER CHECK (file_size >= 0),
    license_id      TEXT,
    license_accepted_at TEXT,
    status          TEXT NOT NULL DEFAULT 'available' CHECK (status IN (
                        'available', 'downloading', 'installed', 'failed', 'removed')),
    error           TEXT,
    is_active       INTEGER NOT NULL DEFAULT 0 CHECK (is_active IN (0, 1)),
    -- Name of this model's sqlite-vec table (created at runtime: dimensions vary per model, ADR 0005).
    vector_table    TEXT UNIQUE,
    installed_at    TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    version         INTEGER NOT NULL DEFAULT 1,
    UNIQUE (model_key, revision),
    CHECK (is_active = 0 OR status = 'installed')
) STRICT;

-- At most one active embedding space.
CREATE UNIQUE INDEX embedding_models_single_active ON embedding_models (is_active) WHERE is_active = 1;

CREATE TRIGGER embedding_models_touch AFTER UPDATE ON embedding_models
FOR EACH ROW WHEN NEW.version = OLD.version
BEGIN
    UPDATE embedding_models SET version = OLD.version + 1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id;
END;

CREATE TABLE embedding_jobs (
    id                 INTEGER PRIMARY KEY,
    document_id        INTEGER NOT NULL REFERENCES documents (id) ON DELETE CASCADE,
    embedding_model_id INTEGER REFERENCES embedding_models (id) ON DELETE SET NULL,
    kind               TEXT NOT NULL CHECK (kind IN ('ingest', 'reembed')),
    stage              TEXT NOT NULL DEFAULT 'queued' CHECK (stage IN (
                           'queued', 'waiting_model', 'extracting', 'structuring', 'chunking',
                           'embedding', 'committing', 'done', 'failed', 'cancelled')),
    progress           REAL NOT NULL DEFAULT 0 CHECK (progress BETWEEN 0 AND 1),
    attempts           INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    error              TEXT,
    started_at         TEXT,
    finished_at        TEXT,
    created_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (stage <> 'failed' OR error IS NOT NULL)
) STRICT;

-- claim_next: oldest unfinished job first.
CREATE INDEX embedding_jobs_pending ON embedding_jobs (created_at)
    WHERE stage NOT IN ('done', 'failed', 'cancelled');
CREATE INDEX embedding_jobs_document ON embedding_jobs (document_id);
CREATE INDEX embedding_jobs_model ON embedding_jobs (embedding_model_id);

CREATE TRIGGER embedding_jobs_touch AFTER UPDATE ON embedding_jobs
FOR EACH ROW WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE embedding_jobs SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id;
END;
