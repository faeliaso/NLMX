# ADR 0006 — Embeddings via `llama-server` supervisionado em loopback

- Status: aceito
- Data: 2026-10-02
- Altera: plano de `embed-llama` via `llama-cpp-2` embutido (TECHNICAL_ARCHITECTURE §2) e a regra "nenhum listener TCP" (ADR 0003 / CLAUDE.md), com uma exceção controlada.

## Contexto
O plano original embutia o llama.cpp no processo do app via `llama-cpp-2` (build com cmake/Metal, versões do crate atrás do llama.cpp, risco R6). O usuário pediu `llama-server` como processo controlado pelo app, em localhost. O binário oficial (`ggml-org/llama.cpp`, build fixado em `scripts/bootstrap.sh`) já vem compilado para Apple Silicon com Metal e expõe `/health`, `/v1/models` e `/v1/embeddings`.

## Decisão
- `crates/adapters/embed-llama` supervisiona um processo filho `llama-server` (`LlamaServer`): start, health, stop (SIGTERM → SIGKILL), restart, timeouts de inicialização/health/requisição, logs em `<data>/logs/llama-server.log` e detecção do processo de uma sessão anterior (pidfile em `<data>/run/`).
- Rede: **somente `127.0.0.1`**, porta efêmera escolhida a cada start, **API key aleatória por execução** passada via `--api-key-file` (arquivo 0600, nunca na linha de comando), `--no-webui`, `--offline`.
- Nunca sinaliza um processo que não seja o binário configurado (verificação por `ps -ww`).
- `LlamaCppEmbeddingProvider` implementa o port `EmbeddingProvider` (`embed`, `embed_batch`, dimensões, identidade do modelo), com reinício automático uma vez se o servidor cair no meio de um lote.
- O modelo GGUF é configuração externa (`embedding.json`), não código.

## Consequências
- Sem build de llama.cpp no `cargo build`; atualizar o llama.cpp é trocar o build fixado.
- Uma porta TCP em loopback fica aberta enquanto o servidor roda. Outros processos do mesmo usuário conseguem conectar, mas não usar a API sem a chave.
- Este build do `llama-server` aceita caminhos de **UNIX socket** em `--host`; migrar para socket eliminaria a porta TCP e é a evolução preferida quando houver necessidade.
- Empacotar `llama-server` + dylibs (resolvidas por `@loader_path`) assinados dentro do `.app` fica para a etapa de Packaging.

## Atualização (2026-10-03)
Empacotamento concluído: `llama-server` vai como sidecar (`bundle.externalBin` → `Contents/MacOS/llama-server`) com rpath `@executable_path/../Frameworks`, e suas dylibs em `Contents/Frameworks`, todas assinadas pelo bundler (`scripts/stage-runtime.sh`, `docs/RELEASE.md`).
