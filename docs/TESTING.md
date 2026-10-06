# Estratégia de testes, métricas e logs

## Suítes

| Alvo | O que roda | Precisa de |
|---|---|---|
| `make test` | todos os testes (padrão) | `make bootstrap` (PDFium) |
| `make test-unit` | testes de unidade dentro de cada crate (`--lib`) | — |
| `make test-integration` | testes de integração dos crates (`crates/*/tests`: SQLite, PDFium, fakes de processo) + doc-tests | PDFium |

Só há testes unitários e de integração, mais `tests/tests/` (regras de arquitetura, ativação de modelo) e `release_acceptance` (só por `make acceptance`, `#[ignore]`). `make test` roda tudo isso; não há suítes E2E, com modelo real, de performance nem benchmark.

## O que cada categoria cobre

- **Unit**: funções puras e componentes isolados, por exemplo:
  - domínio: fusão híbrida, intenções, `Measurement`, `find_in_spans`;
  - aplicação: ContextBuilder, CitationEngine;
  - adaptadores: parser SSE, transformações de layout/rotação do PDFium;
  - telemetria: registro, redação, rotação;
- **Integration**: cada adaptador contra a coisa real ou um fake de processo, por exemplo:
  - SQLite: migrações reversíveis, cascatas, contrato do `ConversationRepository`;
  - PDFium (`crates/adapters/pdf-pdfium/tests`);
  - `fake-llama-server`;
  - servidor HTTP local para downloads.

  Os **contratos dos ports** (`nlmx_testing::*_contract`) rodam contra o fake e contra o adaptador real.
- **PDF**: fixtures geradas por `examples/generate_fixtures.rs`:
  - texto, scanned, misto, relatório, cópia, corrompido;
  - **com senha** (Ghostscript), **páginas rotacionadas**, **acentos em várias fontes**, **200 páginas** e **canário**.

  Os testes verificam metadados, spans com coordenadas, imagens, renderização, erros e rotação (caixas dentro da página exibida, texto achado em páginas giradas).
- **Vector Search**: KNN do sqlite-vec com filtros e reindexação, e **recall@10 ≥ 0,99** contra cosseno exato em 2 000 vetores com semente fixa (`store-sqlite`).
- **RAG**: unidade (contexto, citações, intenções, follow-up).
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
