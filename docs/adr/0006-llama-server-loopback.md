# ADR 0006 — Embeddings via a supervised `llama-server` on loopback

- Status: accepted
- Date: 2026-10-02
- Changes: the plan for `embed-llama` with an embedded `llama-cpp-2` (TECHNICAL_ARCHITECTURE §2) and the "no TCP listener" rule (ADR 0003 / CLAUDE.md), with one controlled exception.

## Context
The original plan embedded llama.cpp in the app process via `llama-cpp-2` (cmake/Metal build, crate versions lagging behind llama.cpp, risk R6). The user asked for `llama-server` as an app-controlled process on localhost. The official binary (`ggml-org/llama.cpp`, build pinned in `scripts/bootstrap.sh`) already comes compiled for Apple Silicon with Metal and exposes `/health`, `/v1/models` and `/v1/embeddings`.

## Decision
- `crates/adapters/embed-llama` supervises a `llama-server` child process (`LlamaServer`): start, health, stop (SIGTERM → SIGKILL), restart, startup/health/request timeouts, logs in `<data>/logs/llama-server.log` and detection of the process from a previous session (pidfile in `<data>/run/`).
- Network: **`127.0.0.1` only**, ephemeral port chosen at each start, **random API key per run** passed via `--api-key-file` (0600 file, never on the command line), `--no-webui`, `--offline`.
- Never signals a process that is not the configured binary (checked via `ps -ww`).
- `LlamaCppEmbeddingProvider` implements the `EmbeddingProvider` port (`embed`, `embed_batch`, dimensions, model identity), with one automatic restart if the server dies in the middle of a batch.
- The GGUF model is external configuration (`embedding.json`), not code.

## Consequences
- No llama.cpp build in `cargo build`; updating llama.cpp means changing the pinned build.
- A loopback TCP port stays open while the server runs. Other processes of the same user can connect, but cannot use the API without the key.
- This `llama-server` build accepts **UNIX socket** paths in `--host`; migrating to a socket would eliminate the TCP port and is the preferred evolution when needed.
- Packaging `llama-server` + dylibs (resolved via `@loader_path`) signed inside the `.app` is left for the Packaging stage.

## Update (2026-10-03)
Packaging done: `llama-server` ships as a sidecar (`bundle.externalBin` → `Contents/MacOS/llama-server`) with rpath `@executable_path/../Frameworks`, and its dylibs in `Contents/Frameworks`, all signed by the bundler (`scripts/stage-runtime.sh`, `docs/RELEASE.md`).

## Update (2026-10-03): memory
- `llama-server` b11349 enables by default an in-RAM prompt cache of up to 8 GiB (`--cache-ram 8192`, `--cache-idle-slots`). With embeddings it only grows, and the process went from ~1 GB to ~10 GB during an import, never releasing it. The server now starts with `--cache-ram 0 --no-cache-prompt --parallel 1`.
- The process is stopped after `idle_shutdown_secs` without use (default 45 s; `0` disables) and starts again on the next request (~0.5 s measured with Qwen3-Embedding-0.6B). We prefer this over the server's own `--sleep-idle-seconds` because stopping the process guarantees all memory (including Metal's) returns to the system.
