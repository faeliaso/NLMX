# ADR 0007 — Fusão híbrida ponderada em vez de RRF

- Status: aceito
- Data: 2026-10-02
- Substitui: a fusão por RRF do ADR 0004.

## Contexto
O plano era fundir BM25 e KNN por Reciprocal Rank Fusion. RRF usa só a posição de cada resultado, então o score final não diz quão perto o trecho está da pergunta. O RAG precisa de um score absoluto para o *relevance gate* ("não encontrei" sem chamar o modelo).

## Decisão
`domain::retrieval::fuse` combina os dois mecanismos por média ponderada de scores normalizados:
- semântico = similaridade cosseno limitada a [0, 1] (absoluta);
- lexical = min-max de −bm25 sobre os candidatos × **quadrado da cobertura** (fração dos termos da pergunta presentes no chunk), para que 1 termo em 3 não receba 100 %;
- final = `(w_s·s + w_l·l) / (w_s + w_l)`, desempate pelo semântico e pelo id. Peso 0 desliga um mecanismo.

## Consequências
- O score semântico mantém significado absoluto; `min_relevance` do RAG é um limiar estável.
- Pesos precisam de calibração; `tests/tests/hybrid_comparison.rs` compara os mecanismos (embedder conceitual sempre, modelo real em `make test-llama`).
