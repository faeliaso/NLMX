# ADR 0003 — In-process HTTP router behind Tauri's custom URI scheme

- Status: accepted
- Date: 2026-10-02

## Context
HTMX makes HTTP requests. A `localhost` server would expose the API to any local process. Tauri 2 offers custom URI schemes served by Rust.

## Decision
- `ui-web` exposes an axum router as a `tower::Service<http::Request>`, with no server or port.
- `app-tauri` registers an async custom scheme and only forwards `http::Request` → router → `http::Response`.
- Streaming (answer tokens, progress) does not go through the protocol: it uses Tauri Channels/Events, with minimal JS; when done, HTMX fetches the final fragment.

## Consequences
- No TCP port open; minimal attack surface.
- UI testable without Tauri (in-process requests); replacing HTMX only affects `ui-web`.
- Custom protocol responses do not stream, so there is a second channel (Channel) to maintain.
- `fetch`/CORS behavior for a custom scheme in WKWebView must be validated (spike 1).

## Update (2026-10-03)
Validated: HTMX 4 works over the `nlmx://` scheme in WKWebView (`src-tauri/src/protocol.rs` forwards via `oneshot`); the CSP is sent as a header by `ui-web`. Answer tokens arrive through the `answer_message` command with a Tauri Channel.
