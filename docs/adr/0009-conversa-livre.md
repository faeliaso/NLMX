# ADR 0009 — Conversa livre e escopo da conversa

- Status: aceito
- Data: 2026-10-04
- Atende: RF18 (escopo), RF22 ("não encontrei"), RF29 (conversa livre).

## Contexto
O Chat só respondia a partir dos documentos: toda pergunta passava pelo RAG, o composer ficava desabilitado sem documentos importados e o seletor oferecia apenas "Todos os documentos" ou um documento. Não era possível só conversar com o modelo. Ao mesmo tempo, a garantia do modo documento (resposta citada, "não encontrei" quando a relevância é baixa, sem chamar o modelo) precisa continuar valendo.

## Decisão
- Uma conversa tem um escopo (`domain::chat::ConversationScope`): `Free` (conversa livre), `Library` (todos os documentos) ou `Document(id)`. Uma conversa nova começa livre. Na migração 0010, `conversations.mode` (`free` | `documents`, padrão `documents` para as conversas que já existiam) e, no modo documento, `conversation_scopes` guarda o documento, como antes.
- **Isolamento total:** a resposta livre vem de `services::free_chat::FreeChat`, que recebe só o `LlmProvider`, sem retriever nem leitor de trechos. A conversa livre nunca consulta a biblioteca nem sugere documentos. "Explique este documento." no modo livre pede para escolher um documento, sem chamar o modelo.
- **O modo fica guardado em cada resposta** (`messages.grounding`: `documents` | `free`), não na conversa. Mudar o escopo vale para as próximas perguntas, e as respostas antigas continuam como foram geradas. Regenerar uma resposta usa o mesmo modo dela.
- **"Responder sem os documentos":** uma resposta `NotFound` do modo documento pode ser gerada de novo pelo modelo sozinho (`ChatService::answer_freely`). A resposta passa a ser livre e recebe a marca "Sem documentos". O escopo da conversa não muda. Só é possível uma vez, e só a partir de uma resposta não encontrada nos documentos.
- **Prompt livre:** instruções fixas, curtas e positivas no `system` (atender ao pedido, inclusive textos criativos; não inventar fatos; não repetir as instruções). O modelo do Mac repetia regras restritivas como resposta ("escolha um documento no seletor") e recusava pedidos comuns, como um poema; por isso a informação de que a conversa não vê os documentos fica só na interface (seletor). Os últimos turnos vão como **turnos reais da conversa** (`GenerationRequest::history` → mensagens `user`/`assistant` do `fm serve`), e o `user` leva só a mensagem atual; tudo passa por `context::neutralize`. Com o histórico escrito como texto dentro do `user`, o modelo repetia respostas anteriores nas novas ("Olá! Não sei o que é o comando FM…") e misturava assuntos antigos num pedido novo. Isso foi reproduzido com o `fm` real: 3 em 3 respostas com o texto; nenhuma com turnos. `fm respond` (fallback) e `fm count-tokens` aceitam um prompt só e recebem `flat_user()`. Cada turno é limitado a 1 500 caracteres, e os turnos mais antigos saem até a contagem exata (`count_tokens`) caber na janela, descontada a reserva da resposta.
- Telemetria: `Measurement::Generated { intent: "free", … }`. Só rótulos, contagens e tempos, como os demais.

## Consequências
- A regra "toda resposta cita trechos `[n]`, e relevância baixa ⇒ não encontrado" passa a valer para o **modo documento**. Uma resposta livre não tem fontes e aparece no seletor ("Conversa livre") ou com a marca "Sem documentos".
- O histórico de uma conversa livre pode conter respostas anteriores do modo documento (depois de uma troca de escopo). Esse texto entra no prompt neutralizado, no turno do usuário, como já acontece com a reescrita de follow-up.
- A remoção de documento (ADR 0008) não muda: respostas livres não têm `citations` nem `message_page_refs`, e só as conversas restritas ao documento removido são excluídas.
- O Chat funciona sem documentos e sem modelo de embeddings. Só o Apple FM indisponível impede respostas, com o aviso que já existia.
