# ADR 0004 — SQLite único com FTS5 e sqlite-vec

- Status: aceito (fusão RRF substituída pelo ADR 0007)
- Data: 2026-10-02

## Contexto
Precisamos de metadados, texto, índice lexical e índice vetorial locais, consistentes entre si e sem serviço externo.

## Decisão
- Um arquivo SQLite (WAL) por biblioteca; SQLite `bundled` e sqlite-vec linkado estaticamente.
- FTS5 com conteúdo externo (`chunks`), tokenizer `unicode61 remove_diacritics 2`, BM25.
- sqlite-vec (`vec0`) para KNN; fusão com BM25 por RRF no `Retriever` (camada de aplicação).
- Uma conexão escritora (actor) + pool de leitura; `commit_document` grava chunks, FTS e vetores numa única transação.
- `LexicalIndex` e `VectorIndex` são ports separados, mesmo implementados pela mesma crate.

## Consequências
- Ingestão atômica por documento; backup = um diretório.
- sqlite-vec é pré-1.0 e faz força bruta: adequado ao MVP (~100k chunks); escala maior pode exigir quantização ou trocar o adapter `VectorIndex`.
- FTS5 não tem stemming em português; o vetor cobre a semântica.

## Atualização (2026-10-03)
- A fusão por RRF foi substituída por fusão ponderada de scores normalizados (ADR 0007).
- O port vetorial chama-se `VectorStore` (não `VectorIndex`). Não há `commit_document` único: `save_extraction` grava páginas, chunks e FTS numa transação, e cada lote de vetores é gravado (vec0 + `chunk_embeddings`) em outra, depois.
