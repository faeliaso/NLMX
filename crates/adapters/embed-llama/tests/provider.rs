//! `LlamaCppEmbeddingProvider` against the fake server binary.

mod common;

use std::{path::PathBuf, sync::Arc, time::Duration};

use common::{data_dir, fake_config};
use nlmx_application::ports::EmbeddingProvider;
use nlmx_domain::embedding::{EmbeddingError, EmbeddingPurpose};
use nlmx_embed_llama::{EmbeddingConfig, LlamaCppEmbeddingProvider, LlamaServer};

fn provider(
    name: &str,
    env: &[(&str, &str)],
    max_batch_inputs: usize,
) -> LlamaCppEmbeddingProvider {
    let dir = data_dir(name);
    let server = fake_config(&dir, env);
    let config = EmbeddingConfig {
        model_id: "fake-embedding-v1".into(),
        model_path: server.model_path.clone(),
        pooling: "mean".into(),
        query_prefix: "query: ".into(),
        passage_prefix: "passage: ".into(),
        context_size: 512,
        batch_size: 512,
        max_batch_inputs,
        gpu_layers: 0,
    };
    LlamaCppEmbeddingProvider::new(config, Arc::new(LlamaServer::new(server)))
}

fn norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

fn texts(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("trecho número {i}")).collect()
}

#[tokio::test]
async fn reports_model_identity_and_dimensions() {
    let p = provider("identity", &[("FAKE_DIMS", "16")], 8);
    assert_eq!(
        p.model_id(),
        "fake-embedding-v1",
        "known without starting the server"
    );
    assert_eq!(p.server().pid().await, None);

    let identity = p.identity().await.unwrap();
    assert_eq!(identity.id, "fake-embedding-v1");
    assert_eq!(identity.file_name, "model.gguf");
    assert_eq!(identity.file_size, 9);
    assert_eq!(
        (
            identity.dimensions,
            identity.context_length,
            identity.parameters
        ),
        (16, 4096, 1234)
    );
    assert_eq!(p.dimensions().await.unwrap(), 16);
    p.shutdown().await;
}

#[tokio::test]
async fn embeds_single_texts_normalized_and_deterministic() {
    let p = provider("single", &[], 8);
    let a = p
        .embed("carência de 180 dias", EmbeddingPurpose::Passage)
        .await
        .unwrap();
    let b = p
        .embed("carência de 180 dias", EmbeddingPurpose::Passage)
        .await
        .unwrap();
    assert_eq!(a.len(), 8);
    assert!((norm(&a) - 1.0).abs() < 1e-5, "L2-normalized");
    assert_eq!(a, b);
    // The purpose selects a different prefix, hence a different vector.
    let q = p
        .embed("carência de 180 dias", EmbeddingPurpose::Query)
        .await
        .unwrap();
    assert_ne!(a, q);
    p.shutdown().await;
}

#[tokio::test]
async fn batches_keep_order_across_requests() {
    let p = provider("batch", &[], 3);
    let inputs = texts(10); // 4 requests of ≤ 3 inputs; the fake answers in reverse order
    let batch = p
        .embed_batch(&inputs, EmbeddingPurpose::Passage)
        .await
        .unwrap();
    assert_eq!(batch.len(), 10);
    for (text, vector) in inputs.iter().zip(&batch) {
        assert_eq!(
            vector,
            &p.embed(text, EmbeddingPurpose::Passage).await.unwrap(),
            "{text}"
        );
        assert!((norm(vector) - 1.0).abs() < 1e-5);
    }
    assert!(
        p.embed_batch(&[], EmbeddingPurpose::Passage)
            .await
            .unwrap()
            .is_empty()
    );
    p.shutdown().await;
}

#[tokio::test]
async fn recovers_when_the_server_crashes_mid_batch() {
    // The fake exits after every request; each following request finds it dead and restarts it.
    let p = provider("crash", &[("FAKE_CRASH_AFTER", "1")], 2);
    let inputs = texts(5);
    let vectors = p
        .embed_batch(&inputs, EmbeddingPurpose::Passage)
        .await
        .unwrap();
    assert_eq!(vectors.len(), 5);
    p.shutdown().await;
}

#[tokio::test]
async fn slow_requests_time_out() {
    let p = provider("slow", &[("FAKE_SLOW_MS", "2000")], 8)
        .with_request_timeout(Duration::from_millis(300));
    p.start().await.unwrap();
    p.identity().await.unwrap(); // /v1/models is not slowed down
    let err = p
        .embed("texto", EmbeddingPurpose::Passage)
        .await
        .unwrap_err();
    assert!(matches!(err, EmbeddingError::Timeout(_)), "{err:?}");
    p.shutdown().await;
}

#[tokio::test]
async fn too_long_inputs_are_reported_as_such() {
    let p = provider("too-long", &[], 8);
    let err = p
        .embed("TOO_LONG", EmbeddingPurpose::Passage)
        .await
        .unwrap_err();
    assert!(matches!(err, EmbeddingError::InputTooLong(_)), "{err:?}");
    p.shutdown().await;
}

#[tokio::test]
async fn an_unavailable_server_is_reported() {
    let dir = data_dir("unavailable");
    let mut server = fake_config(&dir, &[]);
    server.binary = PathBuf::from("/nonexistent/llama-server");
    let config = EmbeddingConfig {
        model_id: "x".into(),
        model_path: server.model_path.clone(),
        pooling: "mean".into(),
        query_prefix: String::new(),
        passage_prefix: String::new(),
        context_size: 512,
        batch_size: 512,
        max_batch_inputs: 4,
        gpu_layers: 0,
    };
    let p = LlamaCppEmbeddingProvider::new(config, Arc::new(LlamaServer::new(server)));
    assert!(matches!(
        p.embed("x", EmbeddingPurpose::Query).await,
        Err(EmbeddingError::Unavailable(_))
    ));
}

#[test]
fn configuration_is_loaded_resolved_and_validated() {
    let dir = data_dir("config");
    std::fs::write(dir.join("m.gguf"), b"GGUF").unwrap();
    let path = dir.join("embedding.json");
    std::fs::write(
        &path,
        r#"{"model_id":"m","model_path":"m.gguf","pooling":"last"}"#,
    )
    .unwrap();
    let config = EmbeddingConfig::load(&path).unwrap();
    assert_eq!(
        config.model_path,
        dir.join("m.gguf"),
        "relative to the config file"
    );
    assert_eq!(
        (
            config.context_size,
            config.batch_size,
            config.max_batch_inputs
        ),
        (8192, 2048, 32)
    );

    for (json, message) in [
        (
            r#"{"model_id":"m","model_path":"missing.gguf","pooling":"last"}"#,
            "modelo não encontrado",
        ),
        (
            r#"{"model_id":"m","model_path":"m.gguf","pooling":"max"}"#,
            "pooling",
        ),
        (
            r#"{"model_id":"","model_path":"m.gguf","pooling":"last"}"#,
            "model_id",
        ),
        (
            r#"{"model_id":"m","model_path":"m.gguf","pooling":"last","typo":1}"#,
            "unknown field",
        ),
    ] {
        std::fs::write(&path, json).unwrap();
        let err = EmbeddingConfig::load(&path).unwrap_err();
        assert!(err.0.contains(message), "{json}: {err}");
    }
}
