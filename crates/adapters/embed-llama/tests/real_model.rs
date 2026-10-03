//! Real llama-server + the configured multilingual GGUF model. Ignored by default because it needs
//! `make bootstrap` and `scripts/fetch-embedding-model.sh`; run with `make test-llama`.

use std::path::Path;

use nlmx_application::ports::EmbeddingProvider;
use nlmx_domain::embedding::EmbeddingPurpose;
use nlmx_embed_llama::{EmbeddingConfig, llama_server_path, provider};

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum() // vectors are normalized
}

#[tokio::test]
#[ignore = "needs runtime/llama and models/Qwen3-Embedding-0.6B-Q8_0.gguf"]
async fn multilingual_embeddings_with_the_real_model() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let config_path = std::env::var_os("NLMX_EMBEDDING_CONFIG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| workspace.join("models/embedding.example.json"));
    let config = EmbeddingConfig::load(&config_path).expect("run scripts/fetch-embedding-model.sh");
    let binary = llama_server_path().expect("run make bootstrap");
    let data_dir = std::env::temp_dir().join(format!("nlmx-llama-real-{}", std::process::id()));

    let provider = provider(config, binary, &data_dir);
    let started = std::time::Instant::now();
    let identity = provider.identity().await.unwrap();
    eprintln!("model ready in {:?}: {identity:?}", started.elapsed());
    assert_eq!(identity.dimensions, 1024);
    assert_eq!(identity.file_name, "Qwen3-Embedding-0.6B-Q8_0.gguf");
    assert!(identity.parameters > 500_000_000);

    let passages: Vec<String> = [
        "A carência para internação hospitalar é de 180 dias a partir da assinatura.",
        "A cobertura inclui consultas ambulatoriais e exames laboratoriais.",
        "O contrato pode ser rescindido mediante aviso prévio de 30 dias.",
    ]
    .map(String::from)
    .to_vec();
    let started = std::time::Instant::now();
    let vectors = provider
        .embed_batch(&passages, EmbeddingPurpose::Passage)
        .await
        .unwrap();
    eprintln!("3 passages in {:?}", started.elapsed());

    for (question, expected) in [
        ("Qual é o prazo de carência para internação?", 0),
        ("Quais exames estão cobertos?", 1),
        ("How can I cancel the contract?", 2),
        ("What is the waiting period for hospitalization?", 0),
    ] {
        let q = provider
            .embed(question, EmbeddingPurpose::Query)
            .await
            .unwrap();
        let scores: Vec<f32> = vectors.iter().map(|v| cosine(&q, v)).collect();
        eprintln!("{question:55} → {scores:.3?}");
        let best = scores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(best, expected, "{question}: {scores:?}");
    }
    provider.shutdown().await;
}
