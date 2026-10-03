-- `[página N]` references of an answer: open the document at that page (highlighting the
-- source that covers it, when there is one).
CREATE TABLE message_page_refs (
    message_id  INTEGER NOT NULL REFERENCES messages (id) ON DELETE CASCADE,
    ordinal     INTEGER NOT NULL CHECK (ordinal >= 1),
    document_id INTEGER NOT NULL REFERENCES documents (id) ON DELETE CASCADE,
    page_number INTEGER NOT NULL CHECK (page_number >= 1),
    source      INTEGER CHECK (source >= 1),
    PRIMARY KEY (message_id, ordinal)
) STRICT;

CREATE INDEX message_page_refs_document ON message_page_refs (document_id);
