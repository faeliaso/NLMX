# ADR 0009 — Free conversation and conversation scope

- Status: accepted
- Date: 2026-10-04
- Addresses: RF18 (scope), RF22 ("not found"), RF29 (free conversation).

## Context
The Chat only answered from the documents: every question went through the RAG, the composer was disabled without imported documents and the selector offered only "Todos os documentos" (all documents) or a single document. It was not possible to just talk to the model. At the same time, the document-mode guarantee (cited answer, "not found" when relevance is low, without calling the model) must keep holding.

## Decision
- A conversation has a scope (`domain::chat::ConversationScope`): `Free` (free conversation), `Library` (all documents) or `Document(id)`. A new conversation starts free. In migration 0010, `conversations.mode` (`free` | `documents`, default `documents` for conversations that already existed) and, in document mode, `conversation_scopes` stores the document, as before.
- **Total isolation:** the free answer comes from `services::free_chat::FreeChat`, which receives only the `LlmProvider`, with no retriever or chunk reader. Free conversation never queries the library nor suggests documents. "Explique este documento." in free mode asks to choose a document, without calling the model.
- **The mode is stored in each answer** (`messages.grounding`: `documents` | `free`), not in the conversation. Changing the scope applies to the next questions, and old answers stay as they were generated. Regenerating an answer uses the same mode as it had.
- **"Responder sem os documentos" (answer without the documents):** a `NotFound` answer from document mode can be generated again by the model alone (`ChatService::answer_freely`). The answer becomes free and gets the "Sem documentos" (no documents) label. The conversation scope does not change. This is possible only once, and only from an answer not found in the documents.
- **Free prompt:** fixed, short, positive instructions in `system` (do what is asked, including creative texts; do not invent facts; do not repeat the instructions). The Mac's model repeated restrictive rules as its answer ("escolha um documento no seletor", i.e. choose a document in the selector) and refused ordinary requests, such as a poem; so the information that the conversation does not see the documents lives only in the interface (selector). The latest turns go as **real conversation turns** (`GenerationRequest::history` → `user`/`assistant` messages of `fm serve`), and `user` carries only the current message; everything goes through `context::neutralize`. With the history written as text inside `user`, the model repeated earlier answers in new ones ("Olá! Não sei o que é o comando FM…") and mixed old topics into a new request. This was reproduced with the real `fm`: 3 out of 3 answers with the text; none with turns. `fm respond` (fallback) and `fm count-tokens` accept a single prompt and receive `flat_user()`. Each turn is limited to 1,500 characters, and the oldest turns are dropped until the exact count (`count_tokens`) fits in the window, minus the answer reserve.
- Telemetry: `Measurement::Generated { intent: "free", … }`. Only labels, counts and durations, like the others.

## Consequences
- The rule "every answer cites passages `[n]`, and low relevance ⇒ not found" now applies to **document mode**. A free answer has no sources and appears in the selector ("Conversa livre", free conversation) or with the "Sem documentos" label.
- The history of a free conversation may contain earlier document-mode answers (after a scope change). That text enters the prompt neutralized, in the user turn, as already happens with the follow-up rewrite.
- Document removal (ADR 0008) does not change: free answers have no `citations` nor `message_page_refs`, and only conversations scoped to the removed document are deleted.
- The Chat works without documents and without an embedding model. Only an unavailable Apple FM prevents answers, with the warning that already existed.
