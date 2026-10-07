# ADR 0002 — Apple Foundation Models via `fm serve` on a Unix socket

- Status: accepted
- Date: 2026-10-02

## Context
FoundationModels is a Swift API. Integrating it would require a Swift↔Rust bridge, Swift linking in the build and async/cancellation handling across languages. macOS 27 ships `/usr/bin/fm`, the official CLI, whose `fm serve` exposes an OpenAI Chat Completions-compatible API (`/health`, `/v1/models`, `/v1/chat/completions` with SSE) over TCP or a Unix socket. Verified on macOS 27.0.1.

## Decision
- Minimum app version: macOS 27.
- No Swift code. The `llm-fm` adapter supervises `fm serve --socket <path>` as a child process and speaks HTTP/SSE over the socket (`reqwest` 0.13 with `ClientBuilder::unix_socket`, already used in the workspace — no need for `hyper` + `hyperlocal`).
- Never use TCP mode (avoids exposure to other processes).
- Fallback: `fm respond --stream` per request, same `LanguageModel` contract.

## Consequences
- Removes the Swift bridge and a major build risk.
- Socket path limited to 104 bytes (`sun_path`); a long path fails silently → assert in the code.
- The `fm` license needs a one-time, explicit acceptance via `sudo fm license` (without it, exit 69); the app guides the user and never accepts it on their behalf.
- The `fm` interface may change with macOS updates → detection via `/health`/`/v1/models` and contract tests.
- Requires child process management (lazy start, health check, restart with backoff, shutdown, orphan cleanup).

## Implementation notes (2026-10-02)
- `fm serve` always responds in SSE (even without `"stream": true`), with `usage` (`prompt_tokens`, `completion_tokens`) only when the request sets `stream_options.include_usage`; guardrail refusals arrive as `event: error` in the middle of the stream with HTTP 200.
- A prompt above the window does not degenerate silently on the current macOS: it fails with HTTP 500 "transcript exceeded the model's context size" (measured 2026-10-07: the window is 8,192 tokens; earlier notes said 4,096 and degenerate output). The app still enforces the limit with `fm count-tokens`.
- Implemented as `FoundationModelsProvider` (`crates/adapters/llm-fm`): compatibility (macOS 27+, native Apple Silicon, `fm` present) is checked first; status is classified (`Incompatible`, `LicenseRequired`, `Unavailable{AppleIntelligenceDisabled | DeviceNotEligible | ModelNotReady | Other}`) with a 30 s cache; **`fm respond --stream` fallback** (instructions in `-i`) when `fm serve` fails to start or drops before answering — `fm respond` writes plain text to stdout and, on a guardrail refusal, exits with code 1 and "Error: …guardrails…" on stderr.
- Swift/Objective-C remain unnecessary: `fm` covers availability, counting, separate instructions, streaming and cancellation.
