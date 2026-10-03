# NLMX — Arquitetura

Como o sistema é hoje. Visão e requisitos: [`PRODUCT.md`](PRODUCT.md). Decisões: [`adr/`](adr/). Regras do dia a dia para quem edita o código: `CLAUDE.md` na raiz.

## 1. Estilo e regras de dependência

**Hexagonal (Ports & Adapters) num Cargo workspace** (ADR 0001): cada camada é uma crate, então a regra de dependência é imposta pelo compilador e verificada por `tests/tests/architecture.rs` sobre o grafo real do `cargo metadata`.

```
                 apps/desktop/src-tauri  (nlmx-desktop: composition root + shell)
                    │                │
          crates/ui-web        crates/adapters/*
                    │                │   implementam ports
                    └──► crates/application ◄──┘   use cases + services + ports (traits)
                                  │
                           crates/domain          entidades, value objects, funções puras
```

- `domain` não depende de nada externo; `application` só de `domain` (nunca de rusqlite, pdfium-render, tauri, axum, askama…).
- **Adapters nunca dependem entre si**; `ui-web` não conhece adapters nem Tauri.
- Nenhum tipo externo atravessa um port: adapters mapeiam para tipos do `domain` e para o enum de erro do port.
- Só `src-tauri/src/wiring.rs` instancia adapters (DI com `Arc<dyn Port>`).
- Todo port tem fake em memória e suíte de contrato reutilizável em `crates/testing`, executada contra o fake e o adapter real.

## 2. Layout

```
crates/
  domain/                 tipos e funções puras: document, ingestion, retrieval (fuse, join_adjacent,
                          jaccard…), rag_intent, viewer (find_in_spans), telemetry (Measurement)
  application/            ports.rs · use_cases/ (DocumentIngestion, EmbedDocuments, ChatService,
                          ViewDocument, GetSystemStatus) · services/ (retrieval, retriever, rag/)
  ui-web/                 router axum in-process, handlers, markdown seguro, view models
  testing/                fakes de todos os ports + suítes de contrato
  adapters/
    pdf-pdfium/           DocumentEngine (thread única do PDFium)
    structure-heuristic/  StructureAnalyzer
    chunker-structural/   Chunker
    embed-llama/          EmbeddingProvider + InferenceRuntime (llama-server supervisionado)
    store-sqlite/         repositórios, LexicalIndex (FTS5), VectorStore (sqlite-vec), migrações
    llm-fm/               LlmProvider (fm serve / fm respond)
    models-catalog/       ModelProvider (catálogo embarcado em catalog/models.json, download)
    fs-library/           FileStore (biblioteca de PDFs, SHA-256)
    telemetry/            logs JSON com redação, MetricsRegistry, sampler de memória/disco
apps/desktop/
  src-tauri/              main, wiring, protocol (nlmx://), commands, tauri.conf.json
  ui/                     templates askama (pages, components), styles (Tailwind 4), scripts (JS mínimo)
tests/                    testes de workspace: arquitetura, E2E, qualidade do RAG, privacidade, bench
```

## 3. Ports (`crates/application/src/ports.rs`)

| Port | Para quê | Adapter |
|---|---|---|
| `DocumentEngine` | abrir PDF, metadados, spans de texto com bbox, imagens, renderizar página | `pdf-pdfium` |
| `FileStore` | hash e cópia do PDF para a biblioteca | `fs-library` |
| `StructureAnalyzer` · `Chunker` · `TokenCounter` | layout → seções/blocos → chunks | `structure-heuristic`, `chunker-structural` |
| `DocumentRepository` | documentos, páginas, `save_extraction` atômico, pendentes | `store-sqlite` |
| `ChunkReader` | resolver filtros, ler chunks de um documento ou por ids | `store-sqlite` |
| `LexicalIndex` · `VectorStore` | BM25 (FTS5) e KNN (vec0), separados mesmo no mesmo banco | `store-sqlite` |
| `ConversationRepository` | conversas, escopo, mensagens, fontes/citações | `store-sqlite` |
| `SettingsRepository` · `StorageDiagnostics` | configurações e tamanhos em disco | `store-sqlite` |
| `EmbeddingProvider` · `EmbeddingSource` | vetores Query/Passage; provider atual (trocável em runtime) | `embed-llama`; `EmbeddingSlot` no wiring |
| `InferenceRuntime` | versão/caminho do llama.cpp do bundle | `embed-llama` |
| `ModelProvider` | catálogo, plano + download confirmado, verificar, ativar, remover, atualizar | `models-catalog` |
| `LlmProvider` | status, contagem exata de tokens, geração com streaming e cancelamento | `llm-fm` |
| `Diagnostics` | métricas agregadas da sessão | `telemetry` |

Streaming e progresso saem de `application` por callbacks (`on_token`, progresso de download) que o shell traduz em Tauri Channels/Events.

## 4. Ingestão

```
seletor ─► import_documents (Tauri) ─► DocumentIngestion::import(path)
  SHA-256 ─► duplicata? (failed/inacabado ⇒ reprocessa) ─► FileStore: <data>/library/<sha>.pdf
  ─► ingest(id): DocumentEngine ─► StructureAnalyzer ─► Chunker
  ─► DocumentRepository::save_extraction (páginas + chunks + FTS numa transação)
  ─► EmbedDocuments: EmbeddingProvider(Passage) ─► VectorStore (vetor + chunk_embeddings numa transação)
```

- Status do documento: `queued → extracting → structuring → chunking → embedding → indexed`, ou `needs_ocr` (sem camada de texto) / `failed` (corrompido, senha). Uma falha nunca afeta outros documentos.
- Nada é gravado antes do `save_extraction`, então repetir é idempotente; no boot, `resume()` reprocessa documentos parados e `embed_pending()` gera os vetores que faltam.
- Sem modelo de embeddings ativo, o documento fica em `embedding` (job `waiting_model`) e a busca é só lexical.
- Mudança na saída do analisador ou do chunker ⇒ incrementar `structure_heuristic::VERSION` / `chunker_structural::VERSION`.

## 5. Busca

**`HybridRetriever`** (`application::services::retrieval`), com `RetrievalOptions { top_k, semantic_weight, lexical_weight, filter }`:

1. Filtros (documento, coleção, página) resolvidos por `ChunkReader::resolve`.
2. Vetorial: `embed(Query)` → `VectorStore::search` (KNN cosseno, filtro `document_id IN` dentro do vec0).
3. Lexical: `LexicalQuery` (termos em minúsculas, sem stopwords PT/EN, cada termo entre aspas — texto do usuário nunca vai cru para o `MATCH`) → `LexicalIndex::search` (BM25).
4. Fusão ponderada (`domain::retrieval::fuse`, ADR 0007): cosseno limitado a [0, 1] + min-max de −bm25 × cobertura² dos termos; desempate pelo semântico e pelo id.
5. Sem modelo (ou se ele falhar) cai para lexical e informa `mode` + `warnings`.

**`Retriever`** (`application::services::retriever`) prepara o contexto do RAG: sobre-amostra (`top_k × 3`), remove duplicatas exatas entre documentos (`metadata.duplicates`), une chunks vizinhos **da mesma seção** sem repetir o overlap, remove quase-duplicatas (Jaccard de trigramas ≥ 0,8 **e** mesmos números), aplica `min_score`, `max_per_document` e Top-K.

## 6. Pergunta e resposta

```
POST /chat/{id}/messages ─► pergunta + resposta "streaming" salvas ─► turno HTML
app.js ─► comando answer_message({messageId, onEvent: Channel})
        ─► ChatService::answer ─► RagEngine::ask ─► {kind:"token"}… {kind:"done"}
done ─► GET /chat/messages/{id} (HTML final)      cancel_answer ─► CancelFlag (parcial salvo)
```

`RagEngine::ask` (`application::services::rag`):

1. **Intenção** (`domain::rag_intent`): pergunta comum → busca; "Explique este documento." → `Overview` (primeiro trecho de cada seção do documento do escopo); "seção N" → `Section(N)` (trechos da seção e subseções, complementados pela busca; sem título correspondente, busca normal). Com histórico, o follow-up é reescrito como pergunta independente por uma chamada curta ao LLM.
2. **Retriever** + **relevance gate**: melhor score < `min_relevance` (0,35) ⇒ `NotFound` **sem chamar o modelo**, mostrando os melhores trechos. (Overview e Section não passam pelo gate.)
3. **`ContextBuilder`** (`rag/context.rs`, puro): orçamento = mín(1 800, janela 4 096 − instruções − pergunta − reserva de resposta 700 − margem 10 %); trechos numerados em blocos `<trecho>`, documentos por relevância e páginas em ordem; duplicatas e trechos contidos descartados.
4. **Isolamento**: instruções fixas só no `system`; texto do documento e pergunta só no `user`, passados por `context::neutralize` (`<`/`>` → `‹`/`›`, sem caracteres de controle).
5. **Contagem exata** com `count_tokens` (`fm count-tokens`); acima do limite, remove o trecho mais fraco e recalcula. O `fm serve` não rejeita prompt longo — degenera —, por isso o limite é garantido aqui.
6. **Geração** com streaming; recusa do guardrail ⇒ `Refused` com os trechos.
7. **`CitationEngine`** (`rag/citations.rs`, puro): `[n]`, `[1, 3]`, `[2–3]` → documento/chunk/páginas/bboxes; números inválidos removidos; `[página N]` resolvido para a fonte que cobre a página. Todas as fontes enviadas ficam em `citations` (`cited` marca as citadas); referências de página em `message_page_refs`.

Saída do modelo só é renderizada por `ui-web/src/markdown.rs` (escapa tudo; só parágrafos, listas, negrito e botões de citação).

## 7. PDF Viewer

Tudo via PDFium, sem pdf.js. `GET /viewer/{doc}?page=N&cite={msg}-{n}` ou `&ref={msg}-{page}` abre ao lado do chat (redimensionável, tela cheia, Esc fecha); `/chat?view={doc}` abre a partir de Documentos.

- Tamanhos das páginas vêm do banco (`document_pages`), sem abrir o PDF.
- `ui/scripts/viewer.js` carrega só as páginas próximas do viewport: PNG em degraus de largura (`/documents/{id}/pages/{n}.png?w=`) e camada de texto (`/viewer/{doc}/pages/{n}/text`, spans transparentes para seleção nativa).
- Busca `/viewer/{doc}/search?q=` (`domain::viewer::find_in_spans`: sem acento/caixa, atravessa spans e hifenização); spans em cache por página em `ViewDocument`.
- Destaques são caixas em pontos PDF (origem no topo-esquerda) convertidas em % da página. Zoom 50–300 %, miniaturas, atalhos ←/→, ⌘+/−/0, ⌘F.

## 8. Processos e threads

| Unidade | Tipo | Dono | Ciclo de vida |
|---|---|---|---|
| UI / event loop | thread principal | Tauri | app |
| Runtime async | tokio multi-thread | Tauri | app |
| PDFium | 1 thread dedicada (não é thread-safe) | `pdf-pdfium` | lazy |
| `llama-server` | processo filho (sidecar), 127.0.0.1, porta efêmera, chave por execução | `embed-llama::LlamaServer` | primeiro uso → ocioso por `idle_shutdown_secs` (45 s) ou `RunEvent::Exit`; sobe de novo na próxima requisição |
| `fm serve --socket` | processo filho, Unix socket | `llm-fm` | primeira geração → `RunEvent::Exit` |
| SQLite | conexão WAL | `store-sqlite` | app |

- **`llama-server`** (ADR 0006): pidfile + arquivo de chave em `<data>/run/`, log em `<data>/logs/llama-server.log`. Um servidor saudável de uma sessão anterior, mesmo binário e modelo, é reaproveitado; um velho é substituído; processos que não são o nosso binário nunca recebem sinal. Roda com `--cache-ram 0 --no-cache-prompt --parallel 1`: o cache de prompts padrão (até 8 GiB) não serve para embeddings e levava o processo de ~1 GB a ~10 GB depois de uma importação. Depois de `idle_shutdown_secs` sem requisições (`embedding.json`, padrão 45, `0` = nunca), o processo é encerrado para devolver a memória; enquanto um `UseGuard` (`LlamaServer::begin_use`, mantido por todo `embed_batch`) estiver vivo, ele não para.
- **`fm`** (ADR 0002): compatibilidade (macOS ≥ 27, arm64 nativo, `fm` presente) checada uma vez; `status()` via `fm available` com cache de 30 s (exit 69 ⇒ `LicenseRequired`). Socket em `$TMPDIR/nlmx-fm-<pid>-<n>.sock` (≤ 103 bytes, senão `/tmp`); reiniciado se morrer; uma geração por vez. Se o `serve` não sobe ou cai antes de responder, usa `fm respond --stream -i <instruções> <prompt>` e só tenta o `serve` de novo após 5 min.

## 9. Modelos

- **Runtime (parte do app):** `llama-server` do llama.cpp, build fixado em `scripts/bootstrap.sh`, vai como sidecar; detectado por `LlamaCppRuntime` (`InferenceRuntime`). Nunca é baixado nem removido.
- **Modelos (sob demanda):** `LocalModelProvider` (`models-catalog`) a partir do catálogo embarcado — revisão upstream fixada em `version` e `url`, tamanho e SHA-256 verificados.
  - Armazenamento: `<data>/models/<id>/<versão>/<arquivo>.gguf` + `manifest.json`; download em andamento como `.part`.
  - **Confirmação obrigatória:** `plan_download`/`plan_update` devolvem um `DownloadPlan` (tamanho, licença, espaço); `download` só aceita `ConfirmedDownload`, criado por `DownloadPlan::confirm()` — chamado apenas pelo comando `download_model` (botão do diálogo).
  - Download com retomada via `Range`, SHA-256 incremental, progresso, cancelamento, checagem de disco (margem: o maior entre 5 % e 256 MB); checksum divergente apaga o `.part`.
  - Verificação rápida (manifest, tamanho, magic `GGUF`) no status; SHA-256 completo em `verify`.
  - Atualização: baixa em pasta nova, verifica, troca o ativo e só então remove a antiga.
  - Ativação grava `<data>/embedding.json`, lido pelo `LlamaCppEmbeddingProvider`. Depois de ativar/remover, `apply_model_change` para o `llama-server` antigo e regera os vetores (todos, se o modelo mudou; senão só os pendentes).

## 10. Banco SQLite

`~/Library/Application Support/dev.nlmx.desktop/nlmx.sqlite3`, migrações em `crates/adapters/store-sqlite/migrations/NNNN_nome/{up,down}.sql` (todas reversíveis, tabelas STRICT):

| Migração | Tabelas |
|---|---|
| 0001 | `app_settings` |
| 0002 | `documents`, `document_pages`, `document_chunks`, `document_chunks_fts` (FTS5, `unicode61 remove_diacritics 2`) |
| 0003 | `collections`, `collection_documents` |
| 0004 | `embedding_models`, `embedding_jobs` |
| 0005 · 0007 | `conversations`, `messages`, `citations`, `conversation_scopes` |
| 0006 | `chunk_embeddings` (chunk → modelo → vetor, com o `content_hash` do chunk) |
| 0008 | `message_page_refs` |

Vetores: uma tabela vec0 `chunk_vectors_<embedding_model_id>` por espaço vetorial (modelo + revisão + dimensão), criada em runtime por `create_index`; rowid = id do chunk (ADR 0005).

## 11. Empacotamento

`.app`/`.dmg` aarch64, `minimumSystemVersion 27.0`: `llama-server` como sidecar em `Contents/MacOS`, dylibs do llama.cpp e `libpdfium` em `Contents/Frameworks`, SQLite e sqlite-vec estáticos no binário, Tailwind compilado no build, HTMX vendorizado. Modelos e `fm` ficam fora do bundle. Detalhes, assinatura e verificação: [`RELEASE.md`](RELEASE.md).

## 12. Substituições previstas

Cada uma é um novo adapter, sem mudança em `application`:

| Componente | Substituto possível |
|---|---|
| PDFium | MuPDF, pdf-rs, processo helper (isolamento de crash) |
| `llama-server` | MLX, Core ML, candle; ou `llama-server` em Unix socket |
| sqlite-vec | usearch, LanceDB, HNSW próprio |
| FTS5 | tantivy |
| `fm serve` | llama.cpp gerador, provedor remoto opt-in |
| HTMX/Tailwind | outra UI (só `ui-web`) |
| Tauri | outro shell (só `src-tauri`) |
