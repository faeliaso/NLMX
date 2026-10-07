# NLMX — Product

Vision, requirements and what is done. How the system is built: [`ARCHITECTURE.md`](ARCHITECTURE.md).

## Vision

**NLMX** is a native macOS app that turns a collection of PDFs into a knowledge base you can query in natural language, with everything running on your own Mac. Documents, indexes and questions never leave the device.

- **Target user:** professionals who deal with many PDFs (standards, contracts, manuals, papers, technical documentation) and cannot or do not want to send that content to the cloud.
- **Value proposition:** ask a question and get a grounded answer, with **clickable citations** that lead to the exact page and passage in the PDF.
- **Principles:**
  1. Truly local-first — works offline after the embedding model is downloaded.
  2. Zero installation friction — one `.dmg`, no Python, Ollama, Docker or terminal.
  3. Verifiable trust — every answer shows where it came from; the app admits when it found nothing.
  4. Native to macOS — uses the system model (Apple Foundation Models) instead of shipping a generative LLM.

## Constraints

- **macOS 27 or later, Apple Silicon.** No Intel, Rosetta, Windows or Linux.
- Direct distribution: `.dmg` signed (Developer ID) and notarized. Mac App Store is out of scope (the sandbox would prevent running `/usr/bin/fm`).
- Generation only through the system's `/usr/bin/fm`; **no Swift code** (ADR 0002). Requires Apple Intelligence enabled and the one-time acceptance of `sudo fm license`, which the app never does on the user's behalf.
- No Python, Ollama, Node or remote server in the runtime. The only network use is user-initiated model downloads.

## Functional requirements

✓ done · ◐ partial · ✗ not done

| ID | Requirement | Status |
|---|---|---|
| **Library** | | |
| RF01 | Import documents (PDF, Markdown, TXT, CSV/TSV, EPUB, DOCX, XLSX) via file picker, drag & drop or folder (recursive), and add **notes** (pasted text, no file; ADR 0018) | ◐ picker and notes only (several files, several formats; ADR 0014) |
| RF02 | Detect duplicates by SHA-256 of the content | ✓ |
| RF03 | Copy the file into the internal library (`<data>/library/<sha>.<ext>`) | ✓ |
| RF04 | List documents with title, pages, size, date and status | ✓ |
| RF05 | Remove a document and all derived data | ✓ together with the Chat history that used it and leaving no trace in the database file (ADR 0008) |
| RF06 | Reindex (e.g. when the embedding model changes) | ✓ automatic when the model changes; manual in Indexação ("Reindexar tudo", "Tentar novamente") |
| **Extraction and structure** | | |
| RF07 | Text per page via PDFium, with bounding boxes | ✓ |
| RF08 | Metadata (title, author, dates) and outline | ◐ no language or outline; `ModDate` rarely comes from pdfium-render |
| RF09 | Normalization: hyphenation, ligatures, repeated headers/footers | ✓ |
| RF10 | Structure: headings/sections, paragraphs, lists | ✓ |
| RF11 | Detect scanned PDFs (OCR out of scope for now) | ✓ detects (`needs_ocr`) |
| RF12 | Chunking by section with overlap and source page/bbox | ✓ |
| **Embeddings and indexes** | | |
| RF13 | Local embeddings with llama.cpp (Metal) and multilingual GGUF | ✓ `llama-server` + Qwen3-Embedding-0.6B |
| RF14 | Vectors in sqlite-vec and text in FTS5, in the same SQLite | ✓ |
| RF15 | Background ingestion that resumes after a restart | ◐ resumes on boot; progress in Indexação; no pause/cancel |
| RF16 | Record the model/version of each vector; never mix spaces | ✓ |
| **Search and questions** | | |
| RF17 | Semantic, lexical and hybrid search | ✓ weighted fusion (ADR 0007) |
| RF18 | Scope: library, document or collection | ◐ free conversation, all documents or one document (ADR 0009); collections only in the backend |
| RF19 | Answer via Apple FM with streaming | ✓ |
| RF20 | `[n]` citations linked to document, location (page, section, range, rows, chapter) and passage, for any format | ✓ (also `[página N]` in PDF; ADR 0015) |
| RF21 | Clicking the citation opens the PDF at the page with the passage highlighted; for other formats it selects the source and shows its information (no viewer) | ✓ (ADR 0016) |
| RF22 | "Not found" when relevance is low, without calling the model | ✓ in document mode, with the "Responder sem os documentos" option |
| RF23 | Saved conversations, with follow-up questions | ✓ |
| RF24 | Transparency: show the passages sent to the model | ✓ source list in the answer |
| RF29 | Free conversation with Apple FM, without documents, as the Chat default | ✓ isolated from the library; answers labelled "Sem documentos" (ADR 0009) |
| **Models** | | |
| RF25 | Embedded catalog of embedding models | ✓ |
| RF26 | Download with confirmation, progress, resume (Range), SHA-256 and disk check | ✓ |
| RF27 | Manual import of a GGUF file (offline installation) | ✗ |
| RF28 | Detect Apple FM and the `fm` license, explaining how to resolve it | ✓ |

Also done, outside the original list: the "Explique este documento." and "seção N" intents, regenerate/cancel answer, viewer with search, text layer, zoom and thumbnails, local diagnostics (metrics and logs without content), `--self-check`.

## Non-functional requirements

| Category | Requirement | Measured (0.1.0, M4 16 GB) |
|---|---|---|
| Privacy | No document data leaves the device; no telemetry | `lsof`: only loopback and the `fm` socket; no automated privacy test (log changes are reviewed by hand, see `CONTRIBUTING.md`) |
| Security | No HTTP server on a TCP port (exception: `llama-server` on 127.0.0.1 with a key, ADR 0006); restrictive CSP; minimal capabilities | ✓ |
| Ingestion | 200-page text PDF indexed in < 60 s | ~16 s (p95, with embeddings) |
| Search | Hybrid < 300 ms | p50 32 ms · p95 41 ms |
| Answer | 1st token < 3 s | p50 0.7 s |
| Responsiveness | The UI never blocks during ingestion | ✓ |
| Robustness | Corrupted/password-protected PDF fails in isolation, with a message | ✓ |
| Integrity | SQLite WAL, reversible migrations, transactional write per document | ✓ |
| Data | Everything in `~/Library/Application Support/dev.nlmx.desktop/` | ✓ |
| Languages | UI in pt-BR; a question in Portuguese finds a passage in English | ✓ |
| Observability | Local JSON logs with rotation, without content | ✓ ([`TESTING.md`](TESTING.md#structured-logs)) |

## Open risks

| Risk | Current mitigation |
|---|---|
| The Apple FM window (8,192 tokens) limits broad questions | Context budget of ~3,500 tokens (passages up to 1,200), exact counting with `fm count-tokens` |
| FM guardrails refuse legitimate content | Refusal shown together with the retrieved passages |
| The `fm` interface changes with macOS updates | Classified status, `fm respond` fallback, contract tests |
| Extraction quality (columns, tables, scans) | Fixtures + golden set; OCR pending |
| sqlite-vec is pre-1.0, brute force | Adequate for ~100k chunks; the `VectorStore` port allows swapping it |
| FTS5 has no stemming for Portuguese | Diacritic removal; the vector covers the semantics |

## Next steps

1. **Public distribution:** Developer ID certificate and `make release` (signing, notarization), then `make acceptance` and a test on a clean Mac.
2. **Requirement gaps:** drag & drop and folder (RF01), manual GGUF import (RF27), collections in the UI (RF18), ingestion pause/cancel (RF15).
3. **Embedding performance:** reduce the `llama-server` context/batch (~1.9 GB and 8.4 embeddings/s today). The server's prompt cache, which reached ~10 GB, is turned off, and the process exits after 45 s idle (ADR 0006).
4. **OCR** for scans (e.g. `fm respond --tool ocr`), preserving page and coordinates.
5. **Quality:** numbered headings repeated and removed as a header; standardized documents that differ only in numbers.
6. **Guided first use:** checklist on first launch (`fm` license, Apple Intelligence, model).

**Out of scope:** Intel/Windows/Linux, an alternative generative LLM, reranker, sync, PDF annotation, Mac App Store.
