//! Privacy: a marker present in a PDF's text, its title and its file name, and in the user's
//! question and the model's answer, must never appear in logs (captured at TRACE, through the
//! app's redacting JSON layer) nor in the measurements. The `#[ignore]`d variant runs the real
//! llama-server and Apple Foundation Models and also scans their own log files.

mod support;

use std::sync::Arc;

use nlmx_application::{
    ports::{CancelFlag, EmbeddingSource, LlmProvider},
    services::{rag::RagOptions, retriever::RetrieverOptions},
};
use nlmx_telemetry::{Format, LogLayer, MemorySink, MetricsLayer, MetricsRegistry};
use nlmx_testing::FakeLlmProvider;
use support::{Library, deterministic_embeddings, fixture, temp_dir};
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt};

const CANARY: &str = "CANARIO-7f3a";

/// Installs the app's logging (JSON + redaction, TRACE) and metrics, once per test binary.
fn capture() -> (MemorySink, MetricsRegistry) {
    let sink = MemorySink::default();
    let metrics = MetricsRegistry::new();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(LogLayer::new(sink.clone(), Format::Json).with_filter(EnvFilter::new("trace")))
            .with(MetricsLayer::new(metrics.clone())),
    )
    .unwrap();
    (sink, metrics)
}

/// Imports the canary PDF under a file name that contains the marker (plus a corrupt copy, to
/// exercise error paths), asks about it, and searches it in the viewer.
async fn exercise(
    dir: std::path::PathBuf,
    embeddings: Arc<dyn EmbeddingSource>,
    llm: Arc<dyn LlmProvider>,
) {
    let named = dir.join(format!("Laudo {CANARY}.pdf"));
    std::fs::copy(fixture("canary.pdf"), &named).unwrap();
    let corrupt = dir.join(format!("{CANARY} corrompido.pdf"));
    std::fs::write(&corrupt, format!("%PDF-1.7\n{CANARY} garbage")).unwrap();

    let library = Library::new(dir.join("data"), embeddings);
    let nlmx_domain::ingestion::ImportOutcome::Imported { id, .. } =
        library.ingestion.import(&named).await
    else {
        panic!("canary import")
    };
    let _ = library.ingestion.import(&corrupt).await;
    let _ = library
        .ingestion
        .import(&dir.join(format!("{CANARY} ausente.pdf")))
        .await;

    let chat = library.chat(
        llm,
        RagOptions {
            retriever: RetrieverOptions {
                min_score: 0.0,
                ..Default::default()
            },
            min_relevance: 0.0,
            ..Default::default()
        },
    );
    let c = chat.start(Some(id)).await.unwrap();
    for question in [
        format!("O que diz o prontuário {CANARY}?"),
        "Explique este documento.".into(),
    ] {
        let (_, answer) = chat.ask(c.id, &question).await.unwrap();
        let m = chat
            .answer(answer, &|_| {}, CancelFlag::default())
            .await
            .unwrap();
        assert!(m.status.is_final());
    }
    let viewer = library.viewer();
    assert!(!viewer.search(id, CANARY).await.unwrap().hits.is_empty());
    let _ = viewer.text_layer(id, 1).await.unwrap();
}

fn assert_clean(sink: &MemorySink, metrics: &MetricsRegistry, extra_files: &[std::path::PathBuf]) {
    let lines = sink.0.lock().unwrap();
    assert!(!lines.is_empty(), "logs were captured");
    let leaks: Vec<&String> = lines.iter().filter(|l| l.contains(CANARY)).collect();
    assert!(leaks.is_empty(), "canary in logs: {leaks:#?}");
    let metrics_json = nlmx_telemetry::snapshot_json(&metrics.snapshot()).to_string();
    assert!(!metrics_json.contains(CANARY));
    for file in extra_files {
        if let Ok(text) = std::fs::read_to_string(file) {
            assert!(!text.contains(CANARY), "canary in {}", file.display());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_from_documents_questions_or_answers_reaches_logs() {
    let (sink, metrics) = capture();
    let llm = Arc::new(
        FakeLlmProvider::available()
            .answering(&format!("O titular {CANARY} tem 30 dias de carência [1].")),
    );
    exercise(temp_dir("canary"), deterministic_embeddings(), llm).await;
    assert!(metrics.snapshot().documents_imported >= 1);
    assert_clean(&sink, &metrics, &[]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "real llama-server + Apple Foundation Models (make test-real)"]
async fn nothing_reaches_logs_with_the_real_models() {
    use nlmx_embed_llama::{EmbeddingConfig, llama_server_path, provider};
    let (sink, metrics) = capture();
    let dir = temp_dir("canary-real");
    let data = dir.join("data");
    let config = EmbeddingConfig::load(&support::root().join("models/embedding.example.json"))
        .expect("scripts/fetch-embedding-model.sh");
    let llama = Arc::new(provider(
        config,
        llama_server_path().expect("make bootstrap"),
        &data,
    ));
    let fm = Arc::new(nlmx_llm_fm::FoundationModelsProvider::system(
        data.join("run"),
    ));
    exercise(
        dir.clone(),
        nlmx_testing::FixedEmbeddingSource::of_arc(llama.clone()),
        fm.clone(),
    )
    .await;
    fm.shutdown().await;
    llama.shutdown().await;
    assert_clean(
        &sink,
        &metrics,
        &[
            data.join("logs/llama-server.log"),
            data.join("run/fm-serve.log"),
        ],
    );
}
