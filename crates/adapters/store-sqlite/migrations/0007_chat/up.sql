-- Chat: how each answer ended, every source given to the model (not only the cited ones) and
-- the document a conversation is about.

-- Lifecycle stays in `status`; `outcome` says how a finished answer ended.
ALTER TABLE messages ADD COLUMN outcome TEXT CHECK (outcome IN (
    'answered', 'not_found', 'refused', 'cancelled', 'error'));

ALTER TABLE citations ADD COLUMN cited INTEGER NOT NULL DEFAULT 1 CHECK (cited IN (0, 1));
ALTER TABLE citations ADD COLUMN page_end INTEGER CHECK (page_end >= 1);
ALTER TABLE citations ADD COLUMN section TEXT;
ALTER TABLE citations ADD COLUMN label TEXT NOT NULL DEFAULT '';
ALTER TABLE citations ADD COLUMN document_title TEXT NOT NULL DEFAULT '';

-- Scope of a conversation (absent = every document). Removing the document widens the scope.
CREATE TABLE conversation_scopes (
    conversation_id INTEGER PRIMARY KEY REFERENCES conversations (id) ON DELETE CASCADE,
    document_id     INTEGER NOT NULL REFERENCES documents (id) ON DELETE CASCADE
) STRICT;

CREATE INDEX conversation_scopes_document ON conversation_scopes (document_id);
