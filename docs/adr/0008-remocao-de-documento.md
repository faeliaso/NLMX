# ADR 0008 — Remoção de documento, com o histórico que o usou

- Status: aceito
- Data: 2026-10-03
- Atende: RF05 (remover documento e todos os dados derivados).

## Contexto
As cascatas do banco (migrações 0002–0008) já apagavam páginas, trechos, FTS5, vetores, jobs, coleções, citações, escopos e referências de página ao apagar um documento. Isso não bastava:
- perguntas e respostas ficavam no histórico, e as respostas muitas vezes transcrevem o documento;
- uma conversa restrita ao documento perdia o escopo em silêncio e passava a buscar em tudo;
- a cópia `<data>/library/<sha>.pdf` continuava no disco;
- um `DELETE` não apaga os bytes: o texto ficava nas páginas livres do SQLite, no WAL e nos segmentos do FTS5.

## Decisão
- `DocumentRepository::remove` faz tudo numa transação:
  - exclui as conversas restritas ao documento (`conversation_scopes`);
  - nas demais, remove cada par pergunta + resposta cuja resposta recebeu algum trecho dele (`citations`, citadas ou não) ou tem `[página N]` dele (`message_page_refs`). A pergunta é a mensagem `user` imediatamente anterior;
  - exclui as conversas que ficaram sem mensagens;
  - apaga o documento, e as cascatas cuidam do resto.
- Rastro físico: `PRAGMA secure_delete = ON` em toda conexão, opção `secure-delete` do FTS5 (migração 0009) e `wal_checkpoint(TRUNCATE)` depois da remoção. Um teste verifica que um texto removido não aparece nos bytes do banco nem do WAL.
- `RemoveDocument` recusa a remoção enquanto o documento está sendo lido (`queued`/`extracting`/`structuring`/`chunking`), apaga a cópia da biblioteca e limpa o cache de texto do visualizador. O arquivo original do usuário nunca é tocado.
- O banco é a fonte da verdade: se o arquivo da biblioteca não puder ser apagado, a remoção vale assim mesmo, e `RemoveDocument::prune_library` (na inicialização) apaga arquivos sem documento. Arquivos alterados há menos de 10 min são poupados, por causa de uma importação em andamento.
- Ids podem ser reaproveitados (`documents.id` não é `AUTOINCREMENT`): as URLs das imagens de página levam `v=<prefixo do SHA-256>`, e o cache de spans é limpo na remoção.
- A UI pede confirmação e mostra o que será apagado: trechos, conversas e pares de pergunta e resposta.

## Consequências
- Remover é definitivo; não há desfazer.
- Uma conversa com vários documentos perde só as trocas que usaram o removido. As respostas seguintes continuam, mesmo que a reescrita de follow-up tenha usado o par removido como contexto.
- `secure_delete` zera as páginas liberadas, um custo de escrita pequeno para os volumes do app.
- Fora do alcance do app: backups (Time Machine) e o arquivo original.
- Os vetores do sqlite-vec não são texto; o vec0 pode manter bytes de vetores removidos em seus blocos até reutilizá-los.
