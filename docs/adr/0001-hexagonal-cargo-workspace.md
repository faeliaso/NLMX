# ADR 0001 — Arquitetura hexagonal em Cargo workspace

- Status: aceito
- Data: 2026-10-02

## Contexto
O app combina várias bibliotecas nativas (PDFium, llama.cpp, SQLite/sqlite-vec) e um processo externo (`fm`), todos com risco de substituição futura. Precisamos que a lógica de ingestão, busca e RAG seja testável sem essas dependências e que trocar uma delas não se propague pelo código.

## Decisão
Ports & Adapters, com uma crate por camada/adapter num Cargo workspace:
`domain` ← `application` (use cases + ports) ← `adapters/*` e `ui-web` ← `app-tauri` (composition root).
Adapters nunca dependem entre si; `application` não importa bibliotecas de infraestrutura; tipos externos não atravessam ports. Fakes e suítes de contrato por port ficam em `testing/`.

## Consequências
- Fronteiras impostas pelo compilador, não por convenção.
- Use cases testáveis com fakes, sem GPU, PDFium ou `fm`.
- Custo: mais crates, mapeamento de tipos em cada adapter, wiring explícito em `app-tauri`.
