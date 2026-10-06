-- Notes (ADR 0018): text pasted by the user, with no file. `documents.format` accepts 'note' and
-- `documents.note_text` holds the text, so re-indexing a note never needs a file. Notes have no
-- original path or library file. Sections, provenance and citations keep their location as JSON.

-- SQLite cannot alter a column's CHECK, and recreating `documents` would have to cope with every
-- foreign key that points at it. Instead the column is replaced: the objects that depend on
-- `format` are dropped, a column with the new CHECK takes its values, and they are put back.
-- `documents_touch` is dropped for the copy so that no row's `version`/`updated_at` changes.

DROP TRIGGER documents_touch;
DROP INDEX documents_format;
ALTER TABLE documents DROP COLUMN previewable;

ALTER TABLE documents ADD COLUMN format_next TEXT NOT NULL DEFAULT 'pdf'
    CHECK (format_next IN ('pdf', 'markdown', 'text', 'csv', 'epub', 'docx', 'xlsx', 'note'));
UPDATE documents SET format_next = format;
ALTER TABLE documents DROP COLUMN format;
ALTER TABLE documents RENAME COLUMN format_next TO format;

ALTER TABLE documents ADD COLUMN previewable INTEGER GENERATED ALWAYS AS (format = 'pdf') VIRTUAL;
CREATE INDEX documents_format ON documents (format);

CREATE TRIGGER documents_touch AFTER UPDATE ON documents
FOR EACH ROW WHEN NEW.version = OLD.version
BEGIN
    UPDATE documents SET version = OLD.version + 1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = NEW.id;
END;

ALTER TABLE documents ADD COLUMN note_text TEXT;
