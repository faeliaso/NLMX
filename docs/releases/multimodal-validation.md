# Validação do suporte multimodal (PDF, Markdown, TXT, CSV, EPUB)

Data: 2026-10-04 · Base de comparação: `259d469` (antes do suporte multiformato) · Máquina: macOS 27, Apple Silicon, Apple FM com licença aceita, Qwen3-Embedding-0.6B local.

**Resultado: o PDF não regrediu.** O caminho de produção do PDF (parser → normalizador → chunker multiformato) produz exatamente o que o caminho legado produzia (145 chunks de 9 PDFs: mesmas páginas, caixas, seções e texto) e o conjunto‑ouro de RAG com modelos reais é idêntico à linha de base (hit@1/hit@5/MRR 1,00; "não encontrado" 1,00; citações válidas 1,00).

## 1. Funcionalidades implementadas
- Importação de PDF, Markdown, TXT, CSV e EPUB por um único pipeline (registry de parsers → normalização → estrutura → chunking → embeddings → SQLite FTS5 + sqlite-vec), com estados por etapa, erros isolados por arquivo, reindexação idempotente e atualização do mesmo documento quando o arquivo muda (ADR 0014).
- Importação em segundo plano: todo arquivo entra na lista na hora (`queued`) e é processado em fila, um por vez, com status real por etapa e toast de resumo.
- RAG com proveniência uniforme por fonte (`SourceReference`/`RetrievedSource`: documento, nome, tipo, chunk, relevância, localização, metadados), rótulo `arquivo · localização`, cabeçalho de prompt por formato (PDF inalterado) e cobertura de documentos no Top‑K (ADR 0015).
- Interface: ícone/formato/status por fonte, citações de qualquer formato clicáveis (PDF abre o viewer na página; os demais abrem "Informações da fonte", sem viewer), `/sources/{id}` (ADR 0016).

## 2. Arquivos alterados
`git diff 259d469 --shortstat`: 166 arquivos, +20 839 / −658 (inclui fixtures). Por área: `crates/domain` (tipos de documento, localização, parse, progresso), `crates/application` (pipeline, parsing, ingestão, retriever, RAG, chat, embeddings), `crates/adapters/{parser-text,parser-epub,normalizer-text,chunker-structural,store-sqlite,fs-library}`, `crates/ui-web` + `apps/desktop/ui` (templates, ícones, CSS, JS), `apps/desktop/src-tauri` (fila de importação, wiring), `crates/testing`, `tests/` e `docs/` (ADRs 0010–0016).
Desta validação: `tests/tests/{pdf_regression_e2e,multiformat_stages_e2e,rag_multiformat_quality,frontend_e2e,performance_multiformat}.rs`, `tests/golden/{rag_multiformat.json,corpus/*}`, `tests/tests/support/*` (a `Library` de teste passou a usar o pipeline de produção), `crates/ui-web/tests/assets.rs`, `crates/store-sqlite` (upgrade 0010→0012), `chunker-structural/src/{split,records}.rs` (correção), `application/src/services/retriever.rs` (cobertura de documentos), `Makefile` (`test-perf`).

## 3. Migrations
- `0011_multiformat` — aditiva: `documents.format/mime_type/metadata/normalizer_version/previewable`, `document_sections`, `chunk_provenance`. Nenhuma linha existente é reescrita; `down.sql` apaga apenas documentos que não são PDF (o esquema antigo não os representa).
- `0012_citation_provenance` — aditiva: `citations.document_type/document_name/locator`. Citação sem `locator` é lida como PDF (página/caixas). `down.sql` apaga só citações de documentos não‑PDF.
- Verificado por teste de upgrade 0010→0012 com dados: linhas de documentos/chunks/citações/embeddings idênticas byte a byte (`version`/`updated_at` intactos), mesma ordem de BM25 e de KNN antes e depois, citação antiga legível como PDF, gatilhos de vetor ainda funcionam, `down` mantém os PDFs e só retira o que não cabe no esquema anterior, `up` de novo restaura os padrões.

## 4. Testes executados
| Alvo | Resultado |
|---|---|
| `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings` | limpos |
| `cargo test --workspace` | 659 passam, 16 ignorados (reais/performance) |
| `make test-real` (llama real, Apple FM real, qualidade e canário reais) | todos passam; `rag_quality` real hit@1/5/MRR 1,00, citações válidas 1,00 (igual à linha de base) |
| `rag_multiformat_quality` real (Qwen3 + Apple FM) | fontes no contexto 1,00; lugar certo 1,00; 8/8 perguntas completas; não‑encontrado 2/2; citações corretas 9/9 |
| `pdf_regression_e2e` | 145 chunks legado × produção idênticos; erros equivalentes; FTS, acentos e KNN do PDF funcionam |
| `multiformat_stages_e2e` | detecção → parser → normalização → estrutura → chunking → embeddings → persistência → retrieval → provenance, nos 5 formatos + entradas difíceis (BOM, Latin‑1, UTF‑16, CSV com quebra, setext, front matter) e recusas |
| `rag_multiformat_quality` (determinístico) | 7 tipos de pergunta (só PDF, só MD, só TXT, só CSV, só EPUB, dois formatos, vários) com proveniência exata: página, heading, intervalo de caracteres, linhas, capítulo |
| `frontend_e2e` + testes do `ui-web` | listas, ícones, formato, status, URLs, citações, painel de informações; **nenhum viewer para não‑PDF** (`/viewer/*`, páginas, texto, busca e imagem recusados); viewer de PDF intacto (páginas, texto, busca, PNG) |
| `crates/ui-web/tests/assets.rs` | todo ícone nomeado existe; SVGs bem formados; CSS só com tokens; `node --check` em `app.js`/`viewer.js`/`ds.js` |
| `make test-perf` (release) | ver abaixo |
| `make bundle` + `verify-bundle.sh` | `.app`/DMG gerados; self‑check (PDFium, SQLite + sqlite‑vec, llama.cpp), assinatura ad‑hoc, só arm64, sem modelos no bundle |

Performance (release, embedder determinístico; pico = memória acima do início):
- Markdown 20 MB, 4 027 seções → 27 966 chunks em 2,1 s, +175 MB.
- TXT 50 MB (um parágrafo de 200 KB + 90 954) → 45 693 chunks em 3,8 s, +417 MB.
- CSV 240 000 linhas → 27 576 chunks em 3,7 s, +218 MB.
- EPUB 400 capítulos + imagem de 30 MB → 8 810 chunks em 1,1 s, +22 MB.
- Acima dos limites (TXT 70 MB, CSV 260 000 linhas): recusados em < 0,3 s ("O arquivo é grande demais"), CSV com +202 MB de pico; zip bomb de 1 GB: recusada em 0,0 s, +0 MB; XHTML com 20 000 níveis: indexado; célula CSV de 40 MB: indexada em 3,1 s (21 577 chunks), +561 MB.
- 200 arquivos importados por 4 registradores concorrentes + 1 processador, com buscas em paralelo: 927 arquivos/s, pior busca 1,7 ms, nenhum erro de banco, 1 vetor por chunk, duplicatas recusadas.
- Telas com 1 000 documentos / 30 000 chunks: Documentos 2 ms (1,3 MB de HTML), Indexação 1 ms, Chat 1 ms, Informações da fonte < 1 ms.

## 5. Problemas encontrados
1. **Lacuna de teste:** quase todos os testes de PDF (incl. o conjunto‑ouro de RAG) rodavam no caminho legado, não no que o app usa.
2. **Corte quadrático:** um valor sem espaços (base64, JSON minificado) numa célula de CSV levava 11,8 s com 1 MB e 45 s com 2 MB (debug) — cada pedaço copiava o resto do valor.
3. **Chunk gigante em TXT/Markdown:** um valor sem espaços de 2 MB virava 1 chunk (~500 mil tokens), impossível de embutir (contexto do modelo 8 192) — o documento ficaria preso em "aguardando embeddings".
4. **Recall multiformato:** numa pergunta cuja resposta está em 4 formatos, o CSV ficava fora das 6 passagens do contexto (rank 8) porque o documento mais forte ocupava as vagas.
5. **Mensagem de erro do PDF corrompido** perdeu o detalhe do motor ("estrutura do arquivo corrompida…" → "o arquivo não é um documento PDF válido").
6. **Custo de remover/reindexar em biblioteca grande:** reindexar o EPUB de 8 810 chunks numa biblioteca de ~110 000 chunks levou 57 s (a importação inicial, 1,1 s). Causa medida: o `delete` do FTS5 com `secure-delete` (exigido pelo ADR 0008) custa ~0,3–0,5 ms/linha e cresce com o tamanho do índice; gatilhos de vetor, `chunk_embeddings` e `provenance` são irrelevantes (< 0,2 s).
7. A tela Documentos gera ~1,3 MB de HTML com 1 000 documentos (cada linha traz seu diálogo de remoção).

## 6. Problemas corrigidos
1 — `Library` de teste e os e2e de PDF agora usam o pipeline de produção (`production_pipeline`), com `Library::legacy` só para comparação; novo `pdf_regression_e2e`.
2 e 3 — `split::words` e `records::split_text` cortam valores sem espaços por caracteres em uma passada (`longest_fit` com galope), sem copiar o resto; testes de tempo linear (2 MB) e de limite de tokens por chunk.
4 — `RetrieverOptions::cover_documents` (padrão ligado): documento com correspondência lexical que ficou fora do Top‑K toma o lugar da passagem mais fraca de um documento com várias; nunca remove a única passagem de um documento; teste de unidade e de qualidade.

## 7. Limitações conhecidas
- Problemas 5, 6 e 7 acima não foram alterados (5: só diagnóstico; 6: inerente ao `secure-delete` do FTS5; 7: sem paginação da lista).
- Limites: texto/Markdown/CSV até 64 MiB lidos em memória (pico de +417 MB num TXT de 50 MB); CSV até 250 000 linhas (`CsvStream` com limite de 5 M linhas existe, mas não está ligado ao pipeline); recusar um CSV de 260 000 linhas ainda lê as linhas até o limite (+202 MB).
- Sem OCR; só PDF tem preview; CSV é indexado como texto por linhas (sem consulta tabular); TXT não refina a localização dentro de um bloco.
- A interface foi validada por testes do roteador e de ativos, e `node --check` nos scripts; **cliques reais, painel lateral, estreitamento, tema e toasts em janela de verdade não foram exercitados** (checklist manual em `make dev`).
- `make acceptance` (instala o DMG e baixa ~640 MB) não foi executado; o bundle foi construído e verificado, mas não instalado.
- Embeddings reais foram medidos em qualidade, não em vazão com bibliotecas grandes (a performance acima usa o embedder determinístico).

## 8. Próximos passos técnicos
1. Apagar documento/reindexar em lote no FTS (por exemplo `rebuild`/`optimize` adiado ou chunks maiores para CSV) e medir com a biblioteca real; avaliar o custo do `secure-delete`.
2. Ligar o `CsvStream` ao pipeline (chunking sob demanda) para CSVs acima de 250 000 linhas e reduzir o pico de memória de TXT/Markdown grandes (leitura em blocos).
3. Paginação/virtualização da lista de Documentos e fragmento menor por linha.
4. Preservar o detalhe do motor nos erros de PDF (`ParseError::Invalid` com causa).
5. Teste de interface em navegador real (WebDriver do Tauri) para cliques, painel lateral e responsividade.
6. Vazão de embeddings reais em bibliotecas grandes (`make bench` com biblioteca mista) e reranker.
7. OCR; pausa/cancelamento da fila de importação; métricas da fila no Diagnóstico.
