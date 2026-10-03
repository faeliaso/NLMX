# ADR 0003 — Router HTTP in-process atrás do custom URI scheme do Tauri

- Status: aceito
- Data: 2026-10-02

## Contexto
HTMX faz requisições HTTP. Um servidor em `localhost` exporia a API a qualquer processo local. O Tauri 2 oferece custom URI schemes atendidos pelo Rust.

## Decisão
- `ui-web` expõe um router axum como `tower::Service<http::Request>`, sem servidor nem porta.
- `app-tauri` registra um custom scheme assíncrono e só encaminha `http::Request` → router → `http::Response`.
- Streaming (tokens da resposta, progresso) não passa pelo protocolo: usa Tauri Channels/Events, com JS mínimo; ao concluir, HTMX busca o fragmento final.

## Consequências
- Nenhuma porta TCP aberta; superfície de ataque mínima.
- UI testável sem Tauri (requisições in-process); trocar HTMX afeta só `ui-web`.
- Respostas do custom protocol não fazem streaming, então existe um segundo canal (Channel) a manter.
- Comportamento de `fetch`/CORS para esquema customizado no WKWebView precisa ser validado (spike 1).

## Atualização (2026-10-03)
Validado: HTMX 4 funciona sobre o esquema `nlmx://` no WKWebView (`src-tauri/src/protocol.rs` encaminha via `oneshot`); a CSP é enviada como cabeçalho pelo `ui-web`. Os tokens da resposta chegam pelo comando `answer_message` com um Tauri Channel.
