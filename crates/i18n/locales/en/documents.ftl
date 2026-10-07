documents-title = Documents
documents-description = Documents in your local library. Nothing leaves this Mac.
documents-import = Import documents
documents-add-note = Add note
documents-library-unavailable = Library unavailable
documents-empty-title = No documents
documents-empty-description = Use Import documents, at the top of the page, to get started (PDF, Markdown, TXT, CSV/TSV, EPUB, DOCX or XLSX), or Add note to paste some text. Files are copied to the local library and split into passages for search.
documents-note-dialog-title = Paste the copied text
documents-note-dialog-description = Paste the copied text below to add it as a source.
documents-note-placeholder = Paste or write the note text here
documents-note-label = Note text
documents-note-insert = Insert
documents-note-name = Note
documents-open = Open
documents-details = Details
documents-actions = Actions
documents-more-actions = More actions for { $title }
documents-more-actions-disabled-title = Available once the document has been read
documents-more-actions-disabled-label = More actions (unavailable while the document is being processed)
documents-remove-menu = Remove…
documents-remove = Remove
documents-remove-dialog-title = Remove “{ $title }”?
documents-pages =
    { $count ->
        [one] { $count } page
       *[other] { $count } pages
    }
documents-chunks =
    { $count ->
        [one] { $count } passage
       *[other] { $count } passages
    }
documents-date = { $month }/{ $day }/{ $year }
documents-status-embedding = Waiting for embeddings
documents-status-indexed = Indexed
documents-status-needs-ocr = No text (OCR)
documents-status-failed = Failed
documents-status-queued = Queued
documents-status-extracting = Reading the file
documents-status-structuring = Structuring
documents-status-chunking = Splitting into passages
documents-removal-base =
    { $chunks ->
        [0] The document's copy in the library will be deleted from this Mac.
        [1] The document's copy in the library, its only passage and the search indexes will be deleted from this Mac.
       *[other] The document's copy in the library, its { $chunks } passages and the search indexes will be deleted from this Mac.
    }
documents-removal-conversations =
    { $count ->
        [one] { $count } conversation will be deleted
       *[other] { $count } conversations will be deleted
    }
documents-removal-turns =
    { $count ->
        [one] { $count } question and answer pair that used it will be removed
       *[other] { $count } question and answer pairs that used it will be removed
    }
documents-removal-history = In Chat, { $items }.
documents-removal-join = { $first } and { $second }
documents-removal-original = The original file is not affected.
documents-notice-removed = Document removed. It and everything derived from it were deleted from this Mac.
documents-notice-remove-failed = Could not remove the document: { $reason }
documents-notice-note-unavailable = Could not add the note: importing is not available.
documents-notice-note-added = Note added. It is being indexed in the background.
documents-picker-title = Import documents
documents-picker-filter = Documents
documents-import-started =
    { $count ->
        [one] Importing { $count } file…
       *[other] Importing { $count } files…
    }
documents-import-imported = { $count } imported
documents-import-duplicates =
    { $count ->
        [one] { $count } was already in the library
       *[other] { $count } were already in the library
    }
documents-import-failed = { $count } failed
documents-import-summary-failure = { $summary } — { $first }
documents-import-internal-register = internal failure while registering the file
documents-import-internal-process = internal failure while processing the file
documents-import-internal-note = internal failure while registering the note
