# ADR 0015 — Fontes multiformato no RAG

- Status: aceito
- Data: 2026-10-04
- Atende: o RAG trata PDF, Markdown, TXT, CSV e EPUB como fontes equivalentes (continua os ADR 0010 e 0014).

## Contexto
A indexação já guardava a `SourceLocation` de cada chunk, mas da recuperação em diante tudo era PDF: `ChunkView`, `Passage`, `Source`, `Citation`, `MessageSource` e a interface só conheciam páginas e caixas (chunks de outros formatos tinham a página 1 de enchimento e eram citados como "p. 1").

## Decisão
- **Proveniência uniforme.** `domain::source::RetrievedSource { reference: SourceReference, document_name, relevance_score, metadata }` acompanha cada resultado: documento, nome do arquivo, tipo (o da localização), chunk, relevância, `SourceLocation` e metadados do chunk. `ChunkView` passa a ter `document_name`, `document_type`, `location` e `metadata` (lidos de `chunk_provenance`; chunk de PDF antigo sem proveniência vira `SourceLocation::Pdf` das colunas de página). Os nomes das variantes continuam os do ADR 0010 (`Pdf` = página+caixas, `Markdown` = caminho de títulos, `Text` = intervalo, `Csv` = linhas, `Epub` = capítulo).
- **Retriever.** `Passage.provenance` leva a localização mesclada dos chunks unidos (`SourceLocation::merge`); busca léxica, semântica, fusão, deduplicação e ranking não mudam: movem a passagem inteira, então a proveniência nunca se perde.
- **Rótulo único:** `"<arquivo> · <localização>"` (`arquitetura.pdf · p. 12`, `arquitetura.md · Embeddings › Normalização, linhas 7–8`, `dados.csv · linhas 120–145`, `livro.epub · cap. 7 — Título`).
- **Prompt.** O cabeçalho do trecho de um PDF continua exatamente como era (`paginas`, `secao`); os demais dizem `tipo` e `localizacao`. Texto de documento continua fora de `system` e neutralizado.
- **Citações.** `Citation.provenance`; `viewer_target()` só existe para PDF; `[página N]` só resolve contra fontes PDF (nunca pelo atalho "único documento" de um arquivo sem páginas); `DocumentRef.pages` só para PDF.
- **Persistência.** Migração `0012_citation_provenance` (aditiva): `citations.document_type`, `document_name`, `locator` (JSON `SourceLocation`). Citação sem `locator` é de PDF e é lida das colunas de página/caixas como antes. `MessageSource` ganha `reference` e `document_name`.
- **A interface decide a apresentação.** O RAG só fornece proveniência. PDF abre no visualizador; Markdown/TXT/CSV/EPUB mostram nome, formato, localização e trecho (não clicáveis) e o marcador `[n]` deles não é um botão.

## Consequências
- O rótulo de fonte de respostas novas usa o nome do arquivo, não o título do documento; respostas antigas mantêm o rótulo gravado.
- Não há visualização nem destaque dentro de arquivos que não são PDF.
- O ranking continua sendo a fusão ponderada do ADR 0007; não há reranker novo.
