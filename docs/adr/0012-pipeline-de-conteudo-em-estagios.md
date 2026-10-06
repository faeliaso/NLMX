# ADR 0012 — Pipeline de conteúdo em estágios

- Status: aceito
- Data: 2026-10-04
- Atende: normalização e chunking para PDF, Markdown, TXT, CSV e EPUB (continua os ADR 0010 e 0011).

## Contexto
O chunking de PDF vivia em `chunker-structural` (página e caixas), o de CSV num chunker solto, e cada parser limpava o texto à sua maneira. Para indexar vários formatos sem duplicar lógica nos parsers, cada estágio precisa ter uma responsabilidade só e a proveniência (página, linhas, offsets, capítulo, caminho de títulos) precisa chegar intacta ao chunk.

## Decisão
- **Estágios explícitos, cada um atrás do seu port:** *parser* extrai e estrutura (`DocumentParser` → `ParsedDocument`), *normalizer* limpa o texto (`DocumentNormalizer`), *chunker* corta (`DocumentChunker` → `DocumentChunk`) e *embedding* gera vetores (`EmbedDocuments`, depois que os chunks são gravados). `application::services::pipeline::ContentPipeline` compõe os três primeiros (`parse → normalize → chunk`) e devolve `ProcessedDocument` (metadados, páginas, chunks, avisos e a versão de cada estágio). Ele não usa nada de embeddings (`architecture.rs` confere).
- **Chunk completo** (`domain::parsed::DocumentChunk`): `document_id`, `chunk_id` (`None` até ser gravado), `index` (posição), texto, `token_count` (quando há contador), `section_path` ("Capítulo 3" › "Embeddings" › …), `location: SourceLocation`, `metadata` (`ChunkMetadata`: tipo, título, nome do arquivo, idioma, colunas) e hash do conteúdo. O caminho de seções fica no chunk e não no texto: o estágio de embeddings o antepõe.
- **Normalizador** (`normalizer-text`): NFC; remove BOM, largura zero, hífen suave e controles; espaços Unicode e NBSP viram espaço; ligaduras são expandidas; o espaço é colapsado por tipo de bloco (código preserva espaços e quebras; tabela e registro preservam as quebras de linha). É puro, idempotente e **nunca altera tipo de bloco nem localização**; só descarta bloco que fica vazio. A limpeza específica de PDF (hifenização, cabeçalho e rodapé) continua em `structure-heuristic`, porque depende de spans e coordenadas.
- **Chunker único:** um núcleo genérico em `chunker-structural` opera sobre os blocos de um `ParsedDocument` e calcula a localização do chunk com `SourceLocation::merge` (união dos blocos de origem) e a da sobreposição com `trailing()`. `MultiFormatChunker` (`DocumentChunker`) usa o núcleo para todos os formatos e o `RecordChunker` para CSV. O `StructuralChunker` de PDF (`Chunker`, em produção) virou um invólucro do mesmo núcleo com saída idêntica à anterior (teste de equivalência contra o algoritmo antigo e contra PDFs reais); a versão `1` foi mantida. Só mudam, fora do PDF, a quebra de código e tabela por linhas (nunca por frases) e a junção de itens de lista com `\n`.
- **Localização por formato:** PDF → páginas e caixas; Markdown → caminho de títulos e linhas; TXT → intervalo `[start, end)` em caracteres do texto decodificado; CSV → linhas de dados; EPUB → capítulo e seção. Um bloco grande dividido em vários chunks **herda o intervalo do bloco** (não há refinamento dentro do bloco); por isso o parser de TXT divide parágrafos de mais de ~2 000 caracteres nas quebras de linha simples.
- Contratos reutilizáveis em `nlmx-testing`: `document_normalizer_contract` e `document_chunker_contract` (índice, `document_id`, localização válida e do tipo do documento, caminho de seção, cobertura de cada bloco por um chunk e nenhuma palavra perdida).

## Consequências
- Nada disso está ligado à `DocumentIngestion`, ao banco ou à interface ainda: a ingestão continua usando `DocumentEngine` → `StructureAnalyzer` → `StructuralChunker`. O ADR 0014 religou a ingestão ao `ContentPipeline` (migração com localização opcional e coluna `locator`, `fs-library` sem `.pdf` fixo, `wiring.rs`, seletor).
- Aplicar o normalizador a PDFs já indexados mudaria o `content_hash` de chunks existentes; por isso o normalizador tem `version` e só entra na ingestão junto com a reindexação.
- `tests/tests/architecture.rs` passa a esperar 12 adapters.
