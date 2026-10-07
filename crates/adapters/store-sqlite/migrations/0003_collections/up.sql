CREATE TABLE collections (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE COLLATE NOCASE CHECK (length(trim(name)) > 0),
    description TEXT,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    version     INTEGER NOT NULL DEFAULT 1
) STRICT;

CREATE TRIGGER collections_touch AFTER UPDATE ON collections
FOR EACH ROW WHEN NEW.version = OLD.version
BEGIN
    UPDATE collections SET version = OLD.version + 1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id;
END;

CREATE TABLE collection_documents (
    collection_id INTEGER NOT NULL REFERENCES collections (id) ON DELETE CASCADE,
    document_id   INTEGER NOT NULL REFERENCES documents (id) ON DELETE CASCADE,
    added_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (collection_id, document_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX collection_documents_document ON collection_documents (document_id);
