//! ChatService: question → placeholder → answer saved with sources; cancellation, failures,
//! regeneration, scope and history; free conversations.

use std::sync::{Arc, Mutex};

use nlmx_application::{
    ports::CancelFlag,
    services::{
        free_chat::{FREE_INSTRUCTIONS, FreeChat},
        rag::{CHOOSE_DOCUMENT_ANSWER, RagEngine, RagOptions, context::NOT_FOUND_ANSWER},
        retrieval::HybridRetriever,
        retriever::{Retriever, RetrieverOptions},
    },
    use_cases::{ChatError, ChatService},
};
use nlmx_domain::{
    chat::{AnswerGrounding, ConversationScope, MessageStatus, Role},
    generation::{LanguageModelStatus, LlmError, UnavailableKind},
};
use nlmx_testing::{
    FakeConversations, FakeCorpus, FakeLlmProvider, FakeVectorStore, FixedEmbeddingSource,
};

fn chat(llm: Arc<FakeLlmProvider>) -> ChatService {
    let corpus = Arc::new(
        FakeCorpus::default()
            .chunk(
                1,
                1,
                2,
                "A carência do plano é de 180 dias para internações.",
            )
            .in_section("3. Prazos")
            .chunk(2, 1, 4, "Consultas não têm carência.")
            .in_section("4. Consultas")
            .chunk(9, 2, 1, "O reembolso de consultas ocorre em 30 dias.")
            .in_section("1. Reembolso"),
    );
    let hybrid = HybridRetriever::new(
        FixedEmbeddingSource::none(),
        Arc::new(FakeVectorStore::default()),
        corpus.clone(),
        corpus.clone(),
    );
    ChatService {
        conversations: Arc::new(FakeConversations::default()),
        rag: Arc::new(RagEngine::new(
            Arc::new(Retriever::new(Arc::new(hybrid))),
            corpus,
            llm.clone(),
        )),
        free: Arc::new(FreeChat::new(llm)),
        options: RagOptions {
            retriever: RetrieverOptions {
                min_score: 0.0,
                ..Default::default()
            },
            min_relevance: 0.1,
            ..Default::default()
        },
    }
}

#[tokio::test]
async fn a_question_becomes_a_saved_answer_with_its_sources() {
    let llm = Arc::new(FakeLlmProvider::available().answering("A carência é de 180 dias [1]."));
    let chat = chat(llm);
    let conversation = chat.start(ConversationScope::Library).await.unwrap();
    let (user, assistant) = chat
        .ask(conversation.id, "  Qual a carência para internações?  ")
        .await
        .unwrap();

    let pending = chat.message(assistant).await.unwrap();
    assert_eq!(pending.status, MessageStatus::Streaming);
    assert_eq!(
        chat.message(user).await.unwrap().content,
        "Qual a carência para internações?"
    );
    assert_eq!(
        chat.conversation(conversation.id)
            .await
            .unwrap()
            .title
            .as_deref(),
        Some("Qual a carência para internações?")
    );

    let streamed = Arc::new(Mutex::new(String::new()));
    let sink = streamed.clone();
    let answer = chat
        .answer(
            assistant,
            &move |t| sink.lock().unwrap().push_str(t),
            CancelFlag::default(),
        )
        .await
        .unwrap();
    assert_eq!(answer.status, MessageStatus::Answered);
    assert_eq!(answer.content, "A carência é de 180 dias [1].");
    assert_eq!(*streamed.lock().unwrap(), answer.content);
    let cited: Vec<_> = answer.sources.iter().filter(|s| s.cited).collect();
    assert_eq!(cited.len(), 1);
    assert_eq!(cited[0].n, 1);
    assert!(!cited[0].quote.is_empty() && !cited[0].label.is_empty());

    // Answering twice is refused: the message is no longer pending.
    assert_eq!(
        chat.answer(assistant, &|_| {}, CancelFlag::default())
            .await
            .unwrap_err(),
        ChatError::NotPending
    );
    let messages = chat.messages(conversation.id).await.unwrap();
    assert_eq!(
        messages.iter().map(|m| m.role).collect::<Vec<_>>(),
        [Role::User, Role::Assistant]
    );
}

#[tokio::test]
async fn empty_questions_are_rejected() {
    let chat = chat(Arc::new(FakeLlmProvider::available()));
    let c = chat.start(ConversationScope::Library).await.unwrap();
    assert_eq!(
        chat.ask(c.id, "   ").await.unwrap_err(),
        ChatError::EmptyQuestion
    );
}

#[tokio::test]
async fn cancelling_saves_the_partial_answer() {
    let llm = Arc::new(
        FakeLlmProvider::available()
            .answering("A carência é de 180 dias [1] para internações.")
            .cancel_after(3),
    );
    let chat = chat(llm);
    let c = chat.start(ConversationScope::Library).await.unwrap();
    let (_, a) = chat.ask(c.id, "carência").await.unwrap();
    let m = chat
        .answer(a, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    assert_eq!(m.status, MessageStatus::Cancelled);
    assert_eq!(m.content, "A carência é");
}

#[tokio::test]
async fn an_unavailable_model_is_a_failure_with_guidance_and_the_sources_found() {
    let llm = Arc::new(FakeLlmProvider::new(LanguageModelStatus::LicenseRequired));
    let chat = chat(llm);
    let c = chat.start(ConversationScope::Library).await.unwrap();
    let (_, a) = chat.ask(c.id, "carência").await.unwrap();
    let m = chat
        .answer(a, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    assert_eq!(m.status, MessageStatus::Failed);
    assert!(m.error.as_deref().unwrap().contains("sudo fm license"));
    assert!(!m.sources.is_empty() && m.sources.iter().all(|s| !s.cited));

    let disabled = Arc::new(FakeLlmProvider::new(LanguageModelStatus::Unavailable {
        kind: UnavailableKind::AppleIntelligenceDisabled,
        reason: "off".into(),
    }));
    let chat = self::chat(disabled);
    let c = chat.start(ConversationScope::Library).await.unwrap();
    let (_, a) = chat.ask(c.id, "carência").await.unwrap();
    let m = chat
        .answer(a, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    assert!(m.error.as_deref().unwrap().contains("Ajustes do Sistema"));
}

#[tokio::test]
async fn refusals_and_generation_errors_are_saved() {
    let chat = chat(Arc::new(
        FakeLlmProvider::available().failing(LlmError::Refused("guardrails".into())),
    ));
    let c = chat.start(ConversationScope::Library).await.unwrap();
    let (_, a) = chat.ask(c.id, "carência").await.unwrap();
    let m = chat
        .answer(a, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    assert_eq!(m.status, MessageStatus::Refused);
    assert_eq!(m.error.as_deref(), Some("guardrails"));

    let chat = self::chat(Arc::new(
        FakeLlmProvider::available().failing(LlmError::Timeout),
    ));
    let c = chat.start(ConversationScope::Library).await.unwrap();
    let (_, a) = chat.ask(c.id, "carência").await.unwrap();
    let m = chat
        .answer(a, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    assert_eq!(m.status, MessageStatus::Failed);
    assert!(m.error.unwrap().contains("não respondeu a tempo"));
}

#[tokio::test]
async fn regenerating_clears_the_answer_for_a_new_generation() {
    let chat = chat(Arc::new(
        FakeLlmProvider::available().answering("Primeira [1]."),
    ));
    let c = chat.start(ConversationScope::Library).await.unwrap();
    let (_, a) = chat.ask(c.id, "carência").await.unwrap();
    assert_eq!(
        chat.regenerate(a).await.unwrap_err(),
        ChatError::NotPending,
        "still streaming"
    );
    chat.answer(a, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    chat.regenerate(a).await.unwrap();
    let m = chat.message(a).await.unwrap();
    assert_eq!(
        (m.status, m.content.as_str()),
        (MessageStatus::Streaming, "")
    );
    assert!(m.sources.is_empty());
    let again = chat
        .answer(a, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    assert_eq!(again.status, MessageStatus::Answered);
}

#[tokio::test]
async fn the_conversation_scope_limits_the_documents() {
    let llm = Arc::new(FakeLlmProvider::available().answering("Resumo [1]."));
    let chat = chat(llm.clone());
    let c = chat.start(ConversationScope::Document(2)).await.unwrap();
    let (_, a) = chat.ask(c.id, "Explique este documento.").await.unwrap();
    let m = chat
        .answer(a, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    assert_eq!(m.status, MessageStatus::Answered);
    assert!(m.sources.iter().all(|s| s.document_id == 2));
    assert!(llm.requests()[0].user.contains("reembolso"));
}

#[tokio::test]
async fn follow_ups_see_earlier_answers() {
    let llm = Arc::new(FakeLlmProvider::available().answering("Consultas não têm carência [1]."));
    let chat = chat(llm.clone());
    let c = chat.start(ConversationScope::Library).await.unwrap();
    let (_, a1) = chat
        .ask(c.id, "Qual a carência para internações?")
        .await
        .unwrap();
    chat.answer(a1, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    let (_, a2) = chat.ask(c.id, "e para consultas?").await.unwrap();
    chat.answer(a2, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    let requests = llm.requests();
    // 1st answer, then rewrite + 2nd answer.
    assert_eq!(requests.len(), 3);
    assert!(
        requests[1]
            .user
            .contains("Usuário: Qual a carência para internações?")
    );
    assert!(requests[1].user.contains("e para consultas?"));
}

async fn answer(
    chat: &ChatService,
    conversation: i64,
    question: &str,
) -> nlmx_domain::chat::Message {
    let (_, a) = chat.ask(conversation, question).await.unwrap();
    chat.answer(a, &|_| {}, CancelFlag::default())
        .await
        .unwrap()
}

#[tokio::test]
async fn a_new_chat_is_a_free_conversation() {
    let llm = Arc::new(FakeLlmProvider::available().answering("Olá! Como posso ajudar?"));
    let chat = chat(llm.clone());
    let c = chat.current().await.unwrap();
    assert_eq!(c.scope, ConversationScope::Free);

    let m = answer(&chat, c.id, "Oi, tudo bem?").await;
    assert_eq!(
        (m.status, m.content.as_str(), m.grounding),
        (
            MessageStatus::Answered,
            "Olá! Como posso ajudar?",
            Some(AnswerGrounding::Free)
        )
    );
    assert!(m.sources.is_empty() && m.page_refs.is_empty());
    let request = &llm.requests()[0];
    assert_eq!(request.system, FREE_INSTRUCTIONS);
    assert!(!request.user.contains("<documentos>"));
}

#[tokio::test]
async fn a_free_conversation_never_reads_the_documents() {
    // The question matches a passage word for word: the library must still not be used.
    let llm = Arc::new(FakeLlmProvider::available().answering("Não tenho acesso aos documentos."));
    let chat = chat(llm.clone());
    let c = chat.start(ConversationScope::Free).await.unwrap();
    let m = answer(&chat, c.id, "Qual a carência do plano para internações?").await;
    assert_eq!(m.status, MessageStatus::Answered);
    assert!(m.sources.is_empty());
    assert!(
        llm.requests()
            .iter()
            .all(|r| !r.user.contains("180 dias") && !r.system.contains("180 dias"))
    );

    // "Explique este documento." needs a document: no generation.
    let m = answer(&chat, c.id, "Explique este documento.").await;
    assert_eq!(
        (m.status, m.content.as_str()),
        (MessageStatus::NotFound, CHOOSE_DOCUMENT_ANSWER)
    );
    assert_eq!(llm.requests().len(), 1);
}

#[tokio::test]
async fn a_free_conversation_gives_the_model_the_earlier_turns() {
    let llm = Arc::new(FakeLlmProvider::available().answering("Resposta."));
    let chat = chat(llm.clone());
    let c = chat.start(ConversationScope::Free).await.unwrap();
    answer(&chat, c.id, "Meu nome é <Ana>.").await;
    answer(&chat, c.id, "Qual é o meu nome?").await;
    let second = &llm.requests()[1];
    assert_eq!(second.history.len(), 1);
    assert_eq!(second.history[0].user, "Meu nome é ‹Ana›.");
    assert_eq!(second.history[0].assistant, "Resposta.");
    assert_eq!(second.user, "Qual é o meu nome?");
}

#[tokio::test]
async fn the_oldest_turns_are_left_out_when_the_prompt_does_not_fit() {
    // Counts: first answer fits; second prompt too long once, then fits without the first turn.
    let llm = Arc::new(
        FakeLlmProvider::available()
            .answering("Resposta.")
            .token_counts(vec![100, 9_000, 200]),
    );
    let chat = chat(llm.clone());
    let c = chat.start(ConversationScope::Free).await.unwrap();
    answer(&chat, c.id, "Primeira pergunta").await;
    let m = answer(&chat, c.id, "Segunda pergunta").await;
    assert_eq!(m.status, MessageStatus::Answered);
    assert!(
        llm.requests()[1].history.is_empty(),
        "the first turn was left out"
    );
    assert_eq!(llm.count_calls(), 3);
}

#[tokio::test]
async fn changing_the_scope_keeps_earlier_answers_as_they_were() {
    let llm = Arc::new(FakeLlmProvider::available().answering("A carência é de 180 dias [1]."));
    let chat = chat(llm);
    let c = chat.start(ConversationScope::Free).await.unwrap();
    let free = answer(&chat, c.id, "Oi").await;
    chat.set_scope(c.id, ConversationScope::Library)
        .await
        .unwrap();
    let grounded = answer(&chat, c.id, "Qual a carência para internações?").await;
    assert_eq!(grounded.grounding, Some(AnswerGrounding::Documents));
    assert!(grounded.sources.iter().any(|s| s.cited));
    assert_eq!(
        chat.message(free.id).await.unwrap().grounding,
        Some(AnswerGrounding::Free)
    );
}

#[tokio::test]
async fn an_answer_not_in_the_documents_can_be_answered_without_them() {
    let llm = Arc::new(FakeLlmProvider::available().answering("Paris é a capital da França."));
    let chat = chat(llm.clone());
    let c = chat.start(ConversationScope::Library).await.unwrap();
    let m = answer(&chat, c.id, "Qual é a capital da França?").await;
    assert_eq!(
        (m.status, m.content.as_str()),
        (MessageStatus::NotFound, NOT_FOUND_ANSWER)
    );
    assert!(llm.requests().is_empty(), "relevance gate: no generation");

    chat.answer_freely(m.id).await.unwrap();
    let pending = chat.message(m.id).await.unwrap();
    assert_eq!(
        (pending.status, pending.grounding),
        (MessageStatus::Streaming, Some(AnswerGrounding::Free))
    );
    let m = chat
        .answer(m.id, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    assert_eq!(
        (m.status, m.content.as_str()),
        (MessageStatus::Answered, "Paris é a capital da França.")
    );
    assert!(m.sources.is_empty());
    assert_eq!(llm.requests()[0].system, FREE_INSTRUCTIONS);
    assert_eq!(
        chat.conversation(c.id).await.unwrap().scope,
        ConversationScope::Library,
        "the conversation keeps its scope"
    );

    // Only a "not found" answer from the documents.
    assert_eq!(
        chat.answer_freely(m.id).await.unwrap_err(),
        ChatError::NotAnswerableFreely
    );
    let (question, pending) = chat.ask(c.id, "Outra pergunta").await.unwrap();
    for id in [question, pending] {
        assert_eq!(
            chat.answer_freely(id).await.unwrap_err(),
            ChatError::NotAnswerableFreely
        );
    }
}

#[tokio::test]
async fn explain_after_leaving_a_document_for_the_library_keeps_that_document() {
    let llm = Arc::new(FakeLlmProvider::available().answering("A carência é de 180 dias [1]."));
    let chat = chat(llm.clone());
    let c = chat.start(ConversationScope::Document(1)).await.unwrap();
    let first = answer(&chat, c.id, "Qual a carência para internações?").await;
    assert!(first.sources.iter().any(|s| s.cited && s.document_id == 1));

    chat.set_scope(c.id, ConversationScope::Library)
        .await
        .unwrap();
    let m = answer(&chat, c.id, "Explique este documento.").await;
    assert_eq!(m.status, MessageStatus::Answered);
    assert!(!m.sources.is_empty() && m.sources.iter().all(|s| s.document_id == 1));

    // A library conversation that never cited a document still has to be told which one.
    let fresh = chat.start(ConversationScope::Library).await.unwrap();
    let m = answer(&chat, fresh.id, "Explique este documento.").await;
    assert_eq!(
        (m.status, m.content.as_str()),
        (MessageStatus::NotFound, CHOOSE_DOCUMENT_ANSWER)
    );
}
