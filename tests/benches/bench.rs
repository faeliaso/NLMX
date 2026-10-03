//! `make bench`: measures the app on a reference library (fixtures + a 200-page PDF) and the
//! golden questions — import time, pages/s, chunks, embeddings/s, retrieval and generation
//! latency, memory, storage and error rate. Uses the real embedding model and Apple Foundation
//! Models when available (reported), deterministic fakes otherwise. Reports only; never fails.

#[path = "../tests/support/mod.rs"]
mod support;

use std::{sync::Arc, time::Instant};

use nlmx_application::{
    ports::{CancelFlag, EmbeddingSource, LlmProvider},
    services::rag::RagOptions,
};
use nlmx_domain::telemetry::{Measurement, Operation};
use nlmx_telemetry::{DataLayout, MetricsLayer, MetricsRegistry, snapshot_json};
use nlmx_testing::FakeLlmProvider;
use support::{Library, deterministic_embeddings, root, temp_dir};
use tracing_subscriber::layer::SubscriberExt;

const CORPUS: &[&str] = &[
    "report.pdf",
    "text.pdf",
    "unicode.pdf",
    "mixed.pdf",
    "large.pdf",
    "encrypted.pdf",
];
const ROUNDS: usize = 2;

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
}

fn ms(v: Option<u64>) -> String {
    v.map_or("—".into(), |v| format!("{v} ms"))
}

#[tokio::main]
async fn main() {
    let metrics = MetricsRegistry::new();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(MetricsLayer::new(metrics.clone())),
    )
    .unwrap();
    let dir = temp_dir("bench");
    let data = dir.join("data");

    // Real models when present.
    let mut llama = None;
    let embeddings: Arc<dyn EmbeddingSource> = match (
        nlmx_embed_llama::EmbeddingConfig::load(&root().join("models/embedding.example.json")),
        nlmx_embed_llama::llama_server_path(),
    ) {
        (Ok(config), Some(binary)) if std::env::var_os("NLMX_BENCH_FAKE").is_none() => {
            let provider = Arc::new(nlmx_embed_llama::provider(config, binary, &data));
            llama = Some(provider.clone());
            nlmx_testing::FixedEmbeddingSource::of_arc(provider)
        }
        _ => deterministic_embeddings(),
    };
    let fm = Arc::new(nlmx_llm_fm::FoundationModelsProvider::system(
        data.join("run"),
    ));
    let real_llm = std::env::var_os("NLMX_BENCH_FAKE").is_none() && {
        use nlmx_application::ports::LlmProvider as _;
        fm.status().await.is_available()
    };
    let llm: Arc<dyn LlmProvider> = if real_llm {
        fm.clone()
    } else {
        Arc::new(FakeLlmProvider::available().answering("Resposta de referência [1]."))
    };
    let layout = DataLayout {
        database: data.join("nlmx.sqlite3"),
        library: data.join("library"),
        models: root().join("models"),
        logs: data.join("logs"),
        run: data.join("run"),
    };
    let mut peak_rss = 0u64;
    let mut sample = |metrics: &MetricsRegistry| {
        let m = layout.sample_resources();
        if let Measurement::Resources {
            rss_bytes,
            llama_rss_bytes,
            fm_rss_bytes,
        } = m
        {
            peak_rss =
                peak_rss.max(rss_bytes + llama_rss_bytes.unwrap_or(0) + fm_rss_bytes.unwrap_or(0));
        }
        metrics.record(m.name(), &m.to_json());
    };

    // Import.
    let started = Instant::now();
    let mut library = Library::new(data.clone(), embeddings);
    for file in CORPUS {
        let outcome = library.ingestion.import(&support::fixture(file)).await;
        if let nlmx_domain::ingestion::ImportOutcome::Imported { id, .. } = outcome {
            library.documents.insert(file.to_string(), id);
        }
        sample(&metrics);
    }
    let import_secs = started.elapsed().as_secs_f64();

    // Questions (golden set), answered end to end.
    let golden: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("tests/golden/rag.json")).unwrap(),
    )
    .unwrap();
    let questions: Vec<String> = golden["questions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q["question"].as_str().unwrap().to_string())
        .collect();
    let rag = library.rag(llm);
    for _ in 0..ROUNDS {
        for q in &questions {
            let _ = rag
                .ask(q, &RagOptions::default(), &|_| {}, CancelFlag::default())
                .await;
            sample(&metrics);
        }
    }
    let storage = layout.sample_storage();
    metrics.record(storage.name(), &storage.to_json());
    fm.shutdown().await;
    if let Some(llama) = &llama {
        llama.shutdown().await;
    }

    // Report.
    let s = metrics.snapshot();
    let op = |o: Operation| s.operation(o).cloned().unwrap();
    let (ingest, embed, retrieve, generate) = (
        op(Operation::Ingest),
        op(Operation::Embed),
        op(Operation::Retrieve),
        op(Operation::Generate),
    );
    let st = s.storage.unwrap_or_default();
    println!(
        "\nNLMX — benchmark ({} rounds × {} questions)",
        ROUNDS,
        questions.len()
    );
    println!(
        "  embeddings: {}",
        if llama.is_some() {
            "Qwen3 (llama.cpp, real)"
        } else {
            "deterministic (fake)"
        }
    );
    println!(
        "  generation: {}",
        if real_llm {
            "Apple Foundation Models (real)"
        } else {
            "scripted (fake)"
        }
    );
    println!("────────────────────────────────────────────────────────────");
    println!(
        "  importação total ............ {import_secs:.2} s ({} documentos, {} falha)",
        s.documents_imported, ingest.failed
    );
    println!(
        "  importação por documento .... p50 {} · p95 {}",
        ms(ingest.p50_ms),
        ms(ingest.p95_ms)
    );
    println!(
        "  páginas processadas/s ....... {:.1}",
        s.pages_per_second.unwrap_or(0.0)
    );
    println!(
        "  chunks gerados .............. {} (de {} páginas)",
        s.chunks, s.pages
    );
    println!(
        "  embeddings/s ................ {:.1} ({} no total)",
        s.embeddings_per_second.unwrap_or(0.0),
        s.embeddings
    );
    println!(
        "  retrieval ................... p50 {} · p95 {} · máx {}",
        ms(retrieve.p50_ms),
        ms(retrieve.p95_ms),
        ms(retrieve.max_ms)
    );
    println!(
        "  geração ..................... p50 {} · p95 {} · 1º token p50 {}",
        ms(generate.p50_ms),
        ms(generate.p95_ms),
        ms(s.first_token_p50_ms)
    );
    println!("  memória (pico, app+filhos) .. {}", mb(peak_rss));
    println!(
        "  armazenamento ............... banco {} · biblioteca {} · total {}",
        mb(st.database_bytes),
        mb(st.library_bytes),
        mb(st.database_bytes + st.library_bytes + st.logs_bytes)
    );
    for o in [&ingest, &embed, &retrieve, &generate] {
        println!(
            "  taxa de erro {:<10} ...... {:.1}% ({}/{})",
            o.operation.as_str(),
            o.error_rate().unwrap_or(0.0) * 100.0,
            o.failed,
            o.total
        );
    }

    let mut report = snapshot_json(&s);
    report["import_secs"] = serde_json::json!(import_secs);
    report["peak_rss_bytes"] = serde_json::json!(peak_rss);
    report["real_embeddings"] = serde_json::json!(llama.is_some());
    report["real_generation"] = serde_json::json!(real_llm);
    report["corpus"] = serde_json::json!(CORPUS);
    let out = root().join("target/bench");
    std::fs::create_dir_all(&out).unwrap();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let path = out.join(format!("report-{stamp}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!(
        "────────────────────────────────────────────────────────────\n  relatório: {}",
        path.strip_prefix(root()).unwrap_or(&path).display()
    );
}
