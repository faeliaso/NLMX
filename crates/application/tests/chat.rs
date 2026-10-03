//! ChatService: question → placeholder → answer saved with sources; cancellation, failures,
//! regeneration, scope and history.

use std::sync::{Arc, Mutex};

use nlmx_application::{
    ports::CancelFlag,
    services::{
        rag::{RagEngine, RagOptions},
        retrieval::HybridRetriever,
        retriever::{Retriever, RetrieverOptions},
    },
    use_cases::{ChatError, ChatService},
};
use nlmx_domain::{
    chat::{MessageStatus, Role},
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
            llm,
        )),
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
    let conversation = chat.current().await.unwrap();
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
    let c = chat.current().await.unwrap();
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
    let c = chat.current().await.unwrap();
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
    let c = chat.current().await.unwrap();
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
    let c = chat.current().await.unwrap();
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
    let c = chat.current().await.unwrap();
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
    let c = chat.current().await.unwrap();
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
    let c = chat.current().await.unwrap();
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
    let c = chat.start(Some(2)).await.unwrap();
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
    let c = chat.current().await.unwrap();
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
