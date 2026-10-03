# ADR 0002 — Apple Foundation Models via `fm serve` em Unix socket

- Status: aceito
- Data: 2026-10-02

## Contexto
O FoundationModels é uma API Swift. Integrá-lo exigiria ponte Swift↔Rust, linkagem Swift no build e tratamento de async/cancelamento entre linguagens. O macOS 27 inclui `/usr/bin/fm`, CLI oficial com `fm serve`, que expõe API compatível com OpenAI Chat Completions (`/health`, `/v1/models`, `/v1/chat/completions` com SSE) via TCP ou Unix socket. Validado em macOS 27.0.1.

## Decisão
- Versão mínima do app: macOS 27.
- Sem código Swift. O adapter `llm-fm` supervisiona `fm serve --socket <path>` como processo filho e fala HTTP/SSE pelo socket (`reqwest` 0.13 com `ClientBuilder::unix_socket`, já usado no workspace — dispensa `hyper` + `hyperlocal`).
- Nunca usar modo TCP (evita exposição a outros processos).
- Fallback: `fm respond --stream` por requisição, mesmo contrato `LanguageModel`.

## Consequências
- Elimina a ponte Swift e um risco grande de build.
- Caminho do socket limitado a 104 bytes (`sun_path`); caminho longo falha silenciosamente → assert no código.
- Licença do `fm` precisa de aceite único e explícito via `sudo fm license` (sem aceite, exit 69); o app guia o usuário e nunca aceita sozinho.
- A interface do `fm` pode mudar com atualizações do macOS → detecção por `/health`/`/v1/models` e testes de contrato.
- Exige gestão do processo filho (start lazy, health check, restart com backoff, shutdown, limpeza de órfãos).

## Notas de implementação (2026-10-02)
- `fm serve` sempre responde em SSE (mesmo sem `"stream": true`), sem `usage`; recusas do guardrail chegam como `event: error` no meio do stream com HTTP 200.
- Prompt acima da janela (4096 tokens) não gera erro: o modelo produz saída degenerada. O limite é garantido pelo app com `fm count-tokens`.
- Implementado como `FoundationModelsProvider` (`crates/adapters/llm-fm`): compatibilidade (macOS 27+, Apple Silicon nativo, `fm` presente) checada antes de tudo; status classificado (`Incompatible`, `LicenseRequired`, `Unavailable{AppleIntelligenceDisabled | DeviceNotEligible | ModelNotReady | Other}`) com cache de 30 s; **fallback `fm respond --stream`** (instruções em `-i`) quando o `fm serve` não sobe ou cai antes de responder — `fm respond` escreve texto puro em stdout e, na recusa do guardrail, sai com código 1 e "Error: …guardrails…" no stderr.
- Swift/Objective-C continuam desnecessários: o `fm` cobre disponibilidade, contagem, instruções separadas, streaming e cancelamento.
