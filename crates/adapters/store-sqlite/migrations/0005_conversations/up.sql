CREATE TABLE conversations (
    id            INTEGER PRIMARY KEY,
    title         TEXT,
    -- Optional retrieval scope; the conversation survives if the collection is removed.
    collection_id INTEGER REFERENCES collections (id) ON DELETE SET NULL,
    archived_at   TEXT,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    version       INTEGER NOT NULL DEFAULT 1
) STRICT;

CREATE INDEX conversations_recent ON conversations (updated_at DESC) WHERE archived_at IS NULL;
CREATE INDEX conversations_collection ON conversations (collection_id);

CREATE TRIGGER conversations_touch AFTER UPDATE ON conversations
FOR EACH ROW WHEN NEW.version = OLD.version
BEGIN
    UPDATE conversations SET version = OLD.version + 1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id;
END;

CREATE TABLE messages (
    id                INTEGER PRIMARY KEY,
    conversation_id   INTEGER NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    role              TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system')),
    content           TEXT NOT NULL DEFAULT '',
    status            TEXT NOT NULL DEFAULT 'complete' CHECK (status IN (
                          'streaming', 'complete', 'cancelled', 'failed', 'no_answer')),
    error             TEXT,
    model             TEXT,
    prompt_tokens     INTEGER CHECK (prompt_tokens >= 0),
    completion_tokens INTEGER CHECK (completion_tokens >= 0),
    created_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
) STRICT;

CREATE INDEX messages_conversation ON messages (conversation_id, created_at);

CREATE TRIGGER messages_touch AFTER UPDATE ON messages
FOR EACH ROW WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE messages SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id;
END;

-- A citation keeps a snapshot (document, page, quote) so re-indexing — which replaces chunks —
-- does not erase answer history; removing the document removes its citations (RF05).
CREATE TABLE citations (
    id          INTEGER PRIMARY KEY,
    message_id  INTEGER NOT NULL REFERENCES messages (id) ON DELETE CASCADE,
    ordinal     INTEGER NOT NULL CHECK (ordinal >= 1),
    chunk_id    INTEGER REFERENCES document_chunks (id) ON DELETE SET NULL,
    document_id INTEGER NOT NULL REFERENCES documents (id) ON DELETE CASCADE,
    page_number INTEGER NOT NULL CHECK (page_number >= 1),
    quote       TEXT NOT NULL,
    bboxes      TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(bboxes)),
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (message_id, ordinal)
) STRICT;

CREATE INDEX citations_chunk ON citations (chunk_id);
CREATE INDEX citations_document ON citations (document_id);
