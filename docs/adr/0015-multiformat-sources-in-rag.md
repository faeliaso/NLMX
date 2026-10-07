# ADR 0015 — Multi-format sources in the RAG

- Status: accepted
- Date: 2026-10-04
- Addresses: the RAG treats PDF, Markdown, TXT, CSV and EPUB as equivalent sources (continues ADRs 0010 and 0014).

## Context
Indexing already stored the `SourceLocation` of each chunk, but from retrieval onwards everything was PDF: `ChunkView`, `Passage`, `Source`, `Citation`, `MessageSource` and the UI only knew pages and boxes (chunks of other formats had a filler page 1 and were cited as "p. 1").

## Decision
- **Uniform provenance.** `domain::source::RetrievedSource { reference: SourceReference, document_name, relevance_score, metadata }` accompanies every result: document, file name, type (the location's), chunk, relevance, `SourceLocation` and chunk metadata. `ChunkView` gains `document_name`, `document_type`, `location` and `metadata` (read from `chunk_provenance`; an old PDF chunk without provenance becomes `SourceLocation::Pdf` from the page columns). Variant names remain those of ADR 0010 (`Pdf` = page+boxes, `Markdown` = heading path, `Text` = range, `Csv` = rows, `Epub` = chapter).
- **Retriever.** `Passage.provenance` carries the merged location of the joined chunks (`SourceLocation::merge`); lexical and semantic search, fusion, deduplication and ranking do not change: they move the whole passage, so provenance is never lost.
- **Single label:** `"<file> · <location>"` (`arquitetura.pdf · p. 12`, `arquitetura.md · Embeddings › Normalização, linhas 7–8`, `dados.csv · linhas 120–145`, `livro.epub · cap. 7 — Título`).
- **Prompt.** The chunk header of a PDF stays exactly as it was (`paginas`, `secao`); the others say `tipo` and `localizacao`. Document text stays out of `system` and neutralized.
- **Citations.** `Citation.provenance`; `viewer_target()` exists only for PDF; `[página N]` (`[page N]`) only resolves against PDF sources (never through the "single document" shortcut of a file without pages); `DocumentRef.pages` only for PDF.
- **Persistence.** Migration `0012_citation_provenance` (additive): `citations.document_type`, `document_name`, `locator` (JSON `SourceLocation`). A citation without `locator` is a PDF one and is read from the page/box columns as before. `MessageSource` gains `reference` and `document_name`.
- **The UI decides the presentation.** The RAG only supplies provenance. PDF opens in the viewer; Markdown/TXT/CSV/EPUB show name, format, location and excerpt (not clickable) and their `[n]` marker is not a button.

## Consequences
- The source label of new answers uses the file name, not the document title; old answers keep the stored label.
- There is no preview or highlight inside non-PDF files.
- Ranking is still the weighted fusion of ADR 0007; there is no new reranker.
