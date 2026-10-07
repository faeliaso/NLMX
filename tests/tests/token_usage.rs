//! Token usage end to end through the application layer: the services pass what the model counted
//! through telemetry, and the session registry keeps the latest counted generation.

use std::sync::{Arc, Mutex};

use nlmx_application::{
    ports::CancelFlag,
    services::{
        free_chat::FreeChat,
        rag::{HistoryTurn, RagOptions},
    },
};
use nlmx_domain::generation::{GenerationUsage, LlmError};
use nlmx_telemetry::{MetricsLayer, MetricsRegistry};
use nlmx_testing::FakeLlmProvider;
use tracing_subscriber::layer::SubscriberExt;

fn registry() -> (MetricsRegistry, tracing::subscriber::DefaultGuard) {
    let registry = MetricsRegistry::new();
    let subscriber = tracing_subscriber::registry().with(MetricsLayer::new(registry.clone()));
    (registry, tracing::subscriber::set_default(subscriber))
}

fn earlier(turns: usize) -> Vec<HistoryTurn> {
    (0..turns)
        .map(|i| HistoryTurn {
            question: format!("pergunta {i}"),
            answer: format!("resposta {i}"),
        })
        .collect()
}

async fn ask(llm: FakeLlmProvider, history: &[HistoryTurn]) -> Result<(), ()> {
    let tokens = Mutex::new(Vec::new());
    FreeChat::new(Arc::new(llm))
        .answer(
            "Qual a capital do Brasil?",
            history,
            &RagOptions::default(),
            &|piece| tokens.lock().unwrap().push(piece.to_string()),
            CancelFlag::default(),
        )
        .await
        .map(|_| ())
        .map_err(|_| ())
}

fn usage(prompt_tokens: u32, completion_tokens: u32) -> GenerationUsage {
    GenerationUsage {
        prompt_tokens,
        completion_tokens,
    }
}

fn answering(usage: GenerationUsage) -> FakeLlmProvider {
    FakeLlmProvider::available()
        .answering("Brasília é a capital.")
        .reporting(usage)
}

fn last(registry: &MetricsRegistry) -> Option<(u32, u32, u32)> {
    registry
        .snapshot()
        .last_generation
        .map(|g| (g.turn, g.usage.prompt_tokens, g.usage.completion_tokens))
}

#[tokio::test]
async fn nothing_to_show_before_the_first_answer() {
    let (registry, _guard) = registry();
    assert_eq!(last(&registry), None);
}

#[tokio::test]
async fn the_counts_and_the_turn_follow_each_answer() {
    let (registry, _guard) = registry();
    ask(answering(usage(101, 19)), &[]).await.unwrap();
    assert_eq!(last(&registry), Some((1, 101, 19)));
    ask(answering(usage(1655, 343)), &earlier(6)).await.unwrap();
    assert_eq!(last(&registry), Some((7, 1655, 343)));
}

#[tokio::test]
async fn an_answer_without_counts_keeps_the_last_counted_one() {
    let (registry, _guard) = registry();
    ask(answering(usage(101, 19)), &[]).await.unwrap();
    ask(
        FakeLlmProvider::available().answering("Brasília."),
        &earlier(1),
    )
    .await
    .unwrap();
    assert_eq!(last(&registry), Some((1, 101, 19)));
}

#[tokio::test]
async fn a_failed_generation_keeps_the_last_counted_one() {
    let (registry, _guard) = registry();
    ask(answering(usage(101, 19)), &[]).await.unwrap();
    let failing = FakeLlmProvider::available().failing(LlmError::Timeout);
    assert!(ask(failing, &earlier(1)).await.is_err());
    assert_eq!(last(&registry), Some((1, 101, 19)));
}

#[tokio::test]
async fn a_cancelled_generation_reports_nothing() {
    let (registry, _guard) = registry();
    ask(
        FakeLlmProvider::available()
            .answering("Uma resposta longa que será interrompida no meio.")
            .reporting(usage(900, 400))
            .cancel_after(2),
        &[],
    )
    .await
    .unwrap();
    assert_eq!(last(&registry), None);
}
