indexing-title = Indexing
indexing-subtitle = Extraction, splitting into passages and embeddings, in the background.
indexing-unavailable-title = Indexing unavailable
indexing-empty-title = No indexed documents
indexing-empty-description = When you import documents, the progress of each step is shown here.
indexing-go-documents = Go to Documents
indexing-index-heading = Search index
indexing-keyword-only-title = Keyword search only
indexing-keyword-only-message = Without an embedding model, questions use lexical search only. Activate a model to include semantic search.
indexing-open-models = Open Models
indexing-embedding-model = Embedding model
indexing-hybrid = Hybrid search
indexing-none = None
indexing-keyword-only-badge = Keywords only
indexing-embed-pending = Generate pending embeddings
indexing-retry-failed = Retry failures
indexing-reindex-all = Reindex all…
indexing-reindex-title = Reindex all documents?
indexing-reindex-description = The vectors of { $documents } ({ $chunks }) will be generated again with the active model. Search keeps working in the meantime.
indexing-reindex-confirm = Reindex
indexing-cancel = Cancel
indexing-attention-heading = Needs attention
indexing-recent-heading = Recently completed
indexing-retry = Try again
indexing-disabled-while-running = Available when the indexing in progress finishes
indexing-stat-documents = Documents
indexing-stat-chunks = Passages
indexing-stat-dimensions = Vector dimensions
indexing-chunks-count =
    { $count ->
        [one] { $count } passage
       *[other] { $count } passages
    }
indexing-attempts-count =
    { $count ->
        [one] { $count } attempt
       *[other] { $count } attempts
    }
indexing-documents-count =
    { $count ->
        [one] { $count } document
       *[other] { $count } documents
    }
indexing-docs-indexed = { $count } indexed
indexing-docs-reading = { $count } being read
indexing-docs-awaiting = { $count } awaiting embeddings
indexing-docs-no-text = { $count } without text
indexing-docs-failed = { $count } failed
indexing-chunks-total = { $count } in total
indexing-chunks-embedded = { $count } with vectors
indexing-chunks-pending = { $count } pending
indexing-detail-waiting-model = Waiting for an embedding model to join semantic search.
indexing-detail-no-embeddings = Embeddings have not been generated yet.
indexing-detail-needs-ocr = No page has selectable text. Text recognition (OCR) is not available yet.
indexing-duration-under-second = under 1 s
indexing-duration-seconds = { $seconds } s
indexing-duration-minutes = { $minutes } min { $seconds } s
