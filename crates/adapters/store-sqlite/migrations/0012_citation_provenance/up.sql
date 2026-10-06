-- A citation keeps the provenance of its passage whatever the format (ADR 0015): the format, the
-- name of the file and the JSON `SourceLocation` (page, headings, text range, rows, chapter).
-- Additive only, no existing row is rewritten: a citation without `locator` is a PDF one, read
-- from `page_number`, `page_end` and `bboxes` exactly as before.
ALTER TABLE citations ADD COLUMN document_type TEXT NOT NULL DEFAULT 'pdf';
ALTER TABLE citations ADD COLUMN document_name TEXT NOT NULL DEFAULT '';
ALTER TABLE citations ADD COLUMN locator TEXT CHECK (locator IS NULL OR json_valid(locator));
