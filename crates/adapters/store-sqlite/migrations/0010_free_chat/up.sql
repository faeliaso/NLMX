-- Free conversation (ADR 0009): a conversation either answers from the documents (scope in
-- `conversation_scopes`, absent = every document) or is an open conversation with the model.
-- Existing conversations keep answering from the documents.
ALTER TABLE conversations ADD COLUMN mode TEXT NOT NULL DEFAULT 'documents'
    CHECK (mode IN ('documents', 'free'));

-- What each answer was generated from; it does not change when the conversation's scope does.
ALTER TABLE messages ADD COLUMN grounding TEXT CHECK (grounding IN ('documents', 'free'));

UPDATE messages SET grounding = 'documents' WHERE role = 'assistant';
