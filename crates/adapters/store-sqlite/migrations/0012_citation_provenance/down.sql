-- Citations of documents that are not PDFs have no page: the previous schema cannot show them.
DELETE FROM citations WHERE document_type <> 'pdf';
ALTER TABLE citations DROP COLUMN locator;
ALTER TABLE citations DROP COLUMN document_name;
ALTER TABLE citations DROP COLUMN document_type;
