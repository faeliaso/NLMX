# ADR 0014 — Pipeline de indexação multiformato

- Status: aceito
- Data: 2026-10-04
- Atende: importar PDF, Markdown, TXT, CSV e EPUB de ponta a ponta (continua os ADR 0010–0013).

## Contexto
Parsers (ADR 0011), pipeline de conteúdo (ADR 0012) e persistência multiformato (ADR 0013) existiam, mas a ingestão em produção só lia PDF: `import` fixava `DocumentType::Pdf`, a biblioteca gravava `<sha>.pdf` e o diálogo só aceitava PDF.

## Decisão
- **Um caminho só, escolhido pelo registry.** `DocumentIngestion` recebe um `ContentPipeline` (`parse → normalize → chunk`); o formato vem de `DocumentType::from_path` e o parser, do `ParserRegistry`. Nenhum `if pdf/markdown/csv` fora dos parsers. Sem parser para a extensão, a importação falha antes de copiar o arquivo. Sem `ContentPipeline` (testes antigos com fakes) a ingestão continua no caminho legado só de PDF.
- **Estados reaproveitados**, sem migração: `queued → extracting (parse) → structuring (normalização) → chunking → embedding → indexed`, mais `failed` e `needs_ocr`. A tela mostra uma fase mais fina (`IngestPhase`: lendo, estrutura, trechos, salvando, embeddings, aguardando modelo, indexado, falhou).
- **Progresso por porta.** `ProgressSink` (`application::ports`) recebe `IngestProgress` (documento, nome do arquivo, formato, fase, fração 0–1, nº de chunks, status). A ingestão informa até salvar (60%); `EmbedDocuments` informa por lote (60–100%). O adaptador do Tauri (`ProgressRelay`) emite o evento `ingest-progress` e guarda o que está em andamento para `ingest_progress`. O nome do arquivo vai só para a interface, nunca para logs.
- **Erros isolados.** Cada arquivo é importado em sua própria tarefa; falha de parser vira `Failed` no documento (`error` com a causa, sem conteúdo nem caminho) e a medição `IngestFailed`; um pânico só falha aquele arquivo. `import_many` expõe o mesmo comportamento para chamadores sem Tauri.
- **Reindexação idempotente.** `reindex(id)` reexecuta tudo, qualquer que seja o status: `save_processed` substitui páginas, chunks e estrutura numa transação (o FTS acompanha por gatilhos) e `EmbedDocuments` apaga os vetores do documento antes de inserir. Ids de documento se mantêm.
- **Arquivo alterado = mesmo documento.** `find_by_original_path` + `replace_source`: se o caminho original já é conhecido e o hash mudou, o documento aponta para a nova cópia, volta a `queued` e é reindexado; a cópia antiga sai da biblioteca e o cache do visualizador é descartado. Mesmo hash em outro caminho continua sendo duplicado. As citações já gravadas guardam o texto citado (`citations.chunk_id` vira `NULL` ao trocar os chunks); para PDF, o destaque no visualizador passa a usar o arquivo novo.
- **Biblioteca multiformato.** `fs-library` grava `<sha>.<ext do arquivo>` (`.bin` sem extensão utilizável), reaproveita uma cópia existente do mesmo hash com outra extensão, e `remove`/`prune` reconhecem qualquer extensão (arquivos `<sha>.pdf` antigos continuam válidos).
- **Desktop.** `wiring.rs` monta o registry (PDF, Markdown, TXT, CSV, EPUB), `TextNormalizer` e `MultiFormatChunker`; o seletor de arquivos usa `ParserRegistry::supported_types()`.

## Consequências
- O caminho legado (`engine`/`analyzer`/`chunker` em `DocumentIngestion`, `save_extraction`) fica só para testes com fakes e pode ser removido numa limpeza futura.
- Se o PDFium não carrega, a ingestão inteira continua indisponível (como antes), e não só o PDF.
- Se a nova versão de um arquivo alterado falha ao processar, os chunks da versão anterior permanecem até um novo `save_processed`; o documento fica `failed` e pode ser reprocessado.
- Não há cancelamento nem pausa de importação em andamento.
