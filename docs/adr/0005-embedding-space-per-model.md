# ADR 0005 — Um espaço vetorial (tabela vec0) por modelo de embedding

- Status: aceito
- Data: 2026-10-02

## Contexto
Vetores de modelos diferentes não são comparáveis, e `vec0` tem dimensão fixa. O usuário pode trocar de modelo de embedding.

## Decisão
- Entidade `EmbeddingSpace{model_id, revision, dims}`; cada uma tem sua tabela `chunks_vec_<space_id>`.
- Exatamente um espaço ativo. Trocar de modelo cria o novo espaço e jobs `reembed` (reaproveitando os chunks, sem reextrair PDFs); a busca segue no espaço antigo até a conclusão; a ativação é atômica e o espaço antigo é removido depois.
- Todo vetor é gravado junto com o seu espaço; consultas sempre informam o espaço.

## Consequências
- Impossível misturar vetores de modelos diferentes.
- Troca de modelo sem indisponibilidade da busca, ao custo de espaço em disco temporário.

## Atualização (2026-10-03)
- Tabela real: `chunk_vectors_<embedding_model_id>` (vec0), criada em runtime por `VectorStore::create_index` a partir de `EmbeddingSpace::from_identity`; a ligação chunk → modelo → vetor fica em `chunk_embeddings` (migração 0006).
- Troca de modelo implementada de forma mais simples: `apply_model_change` para o `llama-server` antigo, marca os documentos indexados como `embedding` e regera todos os vetores no novo espaço. Durante a regeração, a busca usa o novo espaço (parcial) e o lexical; não há troca atômica nem limpeza automática do espaço antigo.
