# ADR 0013 — Persistência multi-formato

- Status: aceito
- Data: 2026-10-04
- Atende: guardar documentos, estrutura, chunks e proveniência de PDF, Markdown, TXT, CSV e EPUB (continua os ADR 0010 a 0012).

## Contexto
O banco só conhecia PDF: `document_chunks.page_start NOT NULL CHECK (>= 1)`, `bboxes`, `documents.pdf_created_at`, nenhum formato, nenhuma estrutura. Afrouxar o `NOT NULL` exigiria recriar `document_chunks`, que é a tabela de conteúdo do FTS5 e a tabela em que pendem os triggers do FTS e os triggers `chunk_vectors_<id>_chunk_delete`, criados em runtime, que apagam as linhas vec0 (vec0 não tem cascata). Recriá-la derrubaria esses triggers e arriscaria os rowids que ligam chunks a vetores.

## Decisão
- **Migração 0011, só aditiva** (`up`: sem `UPDATE`, sem recriar tabela; `down` testado):
  - `documents` ganha `format` (padrão `'pdf'`, com `CHECK`), `mime_type` (NULL nas linhas antigas = o MIME do `format`, lido por `DocumentRecord`), `metadata` (JSON de `DocumentMetadata`), `normalizer_version` e `previewable`, coluna **gerada** (`format = 'pdf'`), que nunca diverge do formato. Nenhuma linha existente é reescrita: o trigger `documents_touch` não dispara, então `version` e `updated_at` dos PDFs não mudam.
  - `document_sections`: só a estrutura (`kind` heading/body, título, nível, `ordinal`, `path` em JSON, `locator` em JSON, `parent_id`). O texto fica nos chunks, sem cópia.
  - `chunk_provenance` (chave `chunk_id`, `section_id`, `locator`, `metadata`): a localização (`SourceLocation` em JSON) e os metadados de cada chunk gravado com `save_processed`.
  - `document_chunks`, FTS, vetores, `chunk_embeddings`, citações e `message_page_refs` **não mudam**.
- **Colunas legadas de página.** Um chunk de PDF mantém `page_start`, `page_end` e `bboxes` em `document_chunks` (o visualizador e as citações leem dali); o `locator` de PDF guarda só o intervalo de páginas. Um chunk de outro formato grava `page_start = page_end = 1` — valor-sentinela que o `NOT NULL CHECK (>= 1)` exige, **sem significado** fora de `documents.format = 'pdf'` — e `bboxes = '[]'`; a localização verdadeira está em `chunk_provenance`. Chunks de PDFs indexados antes desta migração não têm linha em `chunk_provenance`: são lidos como `SourceLocation::Pdf` a partir das colunas de página.
- **Modelo conceitual → schema do projeto:** nome = `title`/`original_filename`; caminho = `original_path`/`library_path`; tamanho = `file_size`; hash = `sha256`; chunk = `document_chunks` (`text`, `ordinal`, `token_count`); `section_id` e `metadata` do chunk em `chunk_provenance`; embedding = `chunk_embeddings` + `embedding_models` (modelo, dimensões) + a tabela vec0 do modelo. Segue a convenção do projeto: `kind` (não `type`) e `ordinal` (não `position`).
- **Portas:** `NewDocument.document_type`, `DocumentRecord.document_type`/`mime_type`, e em `DocumentRepository` os métodos `save_processed(StoredExtraction)`, `chunks_of` e `sections_of`. `save_extraction` (PDF, em produção) continua igual. `save_processed` roda numa transação, recusa chunk de outro formato que o do documento sem gravar nada e substitui tudo o que é derivado do arquivo, então salvar duas vezes dá as mesmas linhas.
- **Idempotência:** `documents.sha256` é `UNIQUE` e o `insert` usa `ON CONFLICT DO NOTHING`: o mesmo conteúdo é o mesmo documento, qualquer nome ou extensão, e o primeiro formato importado vale. Reindexar apaga os chunks antigos; o FTS, as linhas vec0 (triggers de runtime) e `chunk_embeddings` (cascata) acompanham.
- **Busca por páginas:** o filtro de páginas só vale para documentos `format = 'pdf'` (`lexical.rs`); a página-sentinela não casa.
- **`down.sql`** apaga os documentos que não são PDF antes de remover as colunas (a versão anterior não sabe lê-los; a cascata limpa chunks, FTS, vetores e estrutura). Os PDFs ficam intactos.

## Consequências
- Nada grava documentos que não são PDF ainda: a ingestão, o RAG, as citações e a interface continuam só PDF. `chunk_provenance` e `document_sections` esperam a etapa que religa a `DocumentIngestion` ao `ContentPipeline` (`ProcessedDocument::into_stored`).
- Citações de formatos sem página precisam de coluna própria (`citations.page_number` é `NOT NULL CHECK (>= 1)`): migração da etapa de RAG.
- Toda tabela nova que guarda dado derivado de um documento depende de `documents (id) ON DELETE CASCADE`, então `RemoveDocument` a cobre; os testes conferem tabelas, FTS, vetores e os bytes do banco depois de remover.
- Consulta de integridade usada nos testes: nenhum chunk de formato ≠ pdf sem `chunk_provenance`; a página de um chunk não-PDF é a sentinela; o `kind` do `locator` é o `format` do documento; toda linha vec0 tem chunk e linha em `chunk_embeddings`, e vice-versa.
