# Estratégia de testes, métricas e logs

## Suítes

| Alvo | O que roda | Precisa de |
|---|---|---|
| `make test` | tudo que não usa modelo real (padrão) | `make bootstrap` (PDFium) |
| `make test-unit` | testes de unidade dentro de cada crate (`--lib`) | — |
| `make test-integration` | testes de integração dos crates (`crates/*/tests`: SQLite, PDFium, fakes de processo) + doc-tests | PDFium |
| `make test-e2e` | `tests/tests/*_e2e.rs`: fluxos completos sobre os adaptadores reais | PDFium |
| `make test-llama` | `llama-server` real + Qwen3 (embeddings, KNN, comparação híbrida) | `scripts/fetch-embedding-model.sh` |
| `make test-fm` | Apple Foundation Models real (RAG e chat) | macOS 27 + `sudo fm license` |
| `make test-real` | `test-llama` + `test-fm` + qualidade do RAG e canário de privacidade com modelos reais | os dois acima |
| `make test-perf` | arquivos grandes e muitos, memória, recusa de limites/zip bombs, importações concorrentes e telas com biblioteca grande (build release; `NLMX_PERF_SCALE=0.1` para um teste rápido) | PDFium |
| `make bench` | medições numa biblioteca de referência (relatório em `target/bench/`) | PDFium; usa os modelos reais se existirem (`NLMX_BENCH_FAKE=1` força os fakes) |

Os testes com modelo real são `#[ignore]` e só rodam por esses alvos.

## O que cada categoria cobre

- **Unit**: funções puras e componentes isolados, por exemplo:
  - domínio: fusão híbrida, intenções, `Measurement`, `find_in_spans`;
  - aplicação: ContextBuilder, CitationEngine;
  - adaptadores: parser SSE, transformações de layout/rotação do PDFium;
  - telemetria: registro, redação, rotação;
  - UI: Markdown seguro.
- **Integration**: cada adaptador contra a coisa real ou um fake de processo, por exemplo:
  - SQLite: migrações reversíveis, cascatas, contrato do `ConversationRepository`;
  - PDFium (`crates/adapters/pdf-pdfium/tests`);
  - `fake-llama-server`, `fake-fm`;
  - servidor HTTP local para downloads.

  Os **contratos dos ports** (`nlmx_testing::*_contract`) rodam contra o fake e contra o adaptador real.
- **PDF**: fixtures geradas por `examples/generate_fixtures.rs`:
  - texto, scanned, misto, relatório, cópia, corrompido;
  - **com senha** (Ghostscript), **páginas rotacionadas**, **acentos em várias fontes**, **200 páginas** e **canário**.

  Os testes verificam metadados, spans com coordenadas, imagens, renderização, erros e rotação (caixas dentro da página exibida, texto achado em páginas giradas).
- **Vector Search**: KNN do sqlite-vec com filtros e reindexação, e **recall@10 ≥ 0,99** contra cosseno exato em 2 000 vetores com semente fixa (`store-sqlite`).
- **RAG**: unidade (contexto, citações, intenções, follow-up) e o **conjunto-ouro** `tests/golden/rag.json` (`tests/tests/rag_quality.rs`). Mede:
  - hit@1, hit@5 e MRR do retrieval;
  - acerto do "não encontrado";
  - taxa de citações válidas.

  Há limites mínimos no modo determinístico e no modo real.
- **IPC**: a lógica dos comandos Tauri (`run_answer`, `run_cancel`) com fakes cobre:
  - `token…done`;
  - geração duplicada (`running`);
  - cancelamento com texto parcial;
  - erros `{ code, message }`.

  Também há ida e volta pelo runtime de teste do Tauri (`tauri::test`) e testes do protocolo `nlmx://` (`src-tauri/src/protocol.rs`).
- **Model Manager** (`crates/adapters/models-catalog`):
  - catálogo;
  - download com progresso e checksum;
  - retomada por Range;
  - cancelamento;
  - disco insuficiente;
  - corrompido;
  - ativação e remoção;
  - atualização;
  - um download por modelo.

  Download **só com confirmação do usuário**: um doc-test `compile_fail` (não se cria `ConfirmedDownload` sem `confirm()`) e um teste de arquitetura (o único chamador de `.confirm()` é o comando `download_model`).
- **E2E**:
  - `app_e2e` cobre o app inteiro, menos a casca nativa: importação (com uma falha), Documentos, Chat (turno, geração, citação clicável, `[página N]`), viewer (posição, destaque, busca, camada de texto, imagem) e métricas coletadas;
  - `indexing_pipeline_e2e`: arquivo → `indexed` para os 5 formatos com adaptadores reais, erros isolados, progresso, reindexação sem duplicar, arquivo alterado e canário de privacidade multiformato.
  - `rag_multiformat_e2e`: recuperação, ranking, prompt, citações e mensagens salvas dos 5 formatos com proveniência preservada (e canário de privacidade).
  - `pdf_regression_e2e`: o PDF pelo pipeline do app × caminho legado (mesmos chunks, páginas, caixas, seções, texto).
  - `multiformat_stages_e2e`: cada etapa (detecção → provenance) nos 5 formatos e entradas difíceis.
  - `rag_multiformat_quality`: 7 tipos de pergunta com proveniência exata (determinístico; real com `--ignored`).
  - `frontend_e2e`: interface sobre a pilha real, sem viewer para não‑PDF.
  - os demais `*_e2e` cobrem ingestão, retriever, RAG e chat.
- **Privacidade** (`privacy_canary`): um marcador no texto, no título e no nome do arquivo de um PDF, e também na pergunta e na resposta, nunca aparece:
  - nos logs (capturados em TRACE pela camada JSON do app);
  - nas métricas;
  - na versão real, também nos logs do `llama-server` e do `fm serve`.

## Medições

Registradas por `nlmx_application::telemetry::record(Measurement)`, um evento `tracing` com alvo `nlmx::metrics`. O tipo `domain::telemetry::Measurement` só aceita ids, contagens, durações e rótulos fixos, então **não há como registrar conteúdo**.

| Medição | Origem |
|---|---|
| tempo de importação, páginas/s, chunks | `DocumentIngestion` (extração, estrutura, chunks, gravação) |
| embeddings/s | `EmbedDocuments` |
| tempo de retrieval (semântico, lexical, total) | `HybridRetriever` |
| tempo de geração, 1º token, tokens do prompt | `RagEngine` |
| taxa de erro por operação | falhas ÷ total (`*Failed` com `ErrorKind`) |
| memória (RSS do app, `llama-server`, `fm serve`) | `nlmx-telemetry::sampler`, a cada 60 s e ao abrir Diagnóstico |
| armazenamento (banco + WAL, biblioteca, modelos, logs) | idem, também depois de importar |

O `MetricsRegistry` (`crates/adapters/telemetry`) agrega a sessão: contadores, p50/p95/máx, taxas. Ele aparece em **Configurações › Diagnóstico** (com "Copiar diagnóstico", só agregados) e no relatório do `make bench`.

## Logs estruturados

- `~/Library/Application Support/dev.nlmx.desktop/logs/nlmx.jsonl`: uma linha JSON por evento, com `ts`, `level`, `target`, `message`, `fields` e `spans`. Rotação a cada 5 MB, mantém 5 arquivos. Nível por `RUST_LOG` (padrão `info`). Em debug, também em texto no terminal.
- **Política: nunca registrar** texto de documento, trechos, perguntas, respostas, prompts, títulos, nomes de arquivo ou caminhos do usuário. Identifique documentos por `document_id` e erros por tipo.
- **Rede de segurança** (`nlmx-telemetry::redact`):
  - campos chamados `text`, `content`, `quote`, `query`, `question`, `answer`, `prompt`, `path`, `file`, `filename`, `title`… viram `[redigido]`;
  - caminhos `/Users/`, `/Volumes/`, `/private/`, `/var/folders/`, `/tmp/` em mensagens viram `<path>`;
  - textos livres são cortados em 300 caracteres.
- Os logs próprios do `llama-server` (`logs/llama-server.log`) e do `fm serve` (`run/fm-serve.log`) são escritos pelos processos. O canário real verifica que não contêm conteúdo.
