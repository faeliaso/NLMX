//! The `embedding.json` written by the model manager is what the embedding provider reads.

use std::{
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    sync::Arc,
};

use nlmx_application::ports::{CancelFlag, ModelProvider};
use nlmx_domain::models::{License, ModelDescriptor};
use nlmx_embed_llama::{EmbeddingConfig, config_path};
use nlmx_models_catalog::LocalModelProvider;
use sha2::{Digest, Sha256};

#[tokio::test]
async fn an_activated_model_is_a_valid_embedding_configuration() {
    let bytes: Vec<u8> = b"GGUF"
        .iter()
        .copied()
        .chain((0..50_000u32).map(|i| (i % 251) as u8))
        .collect();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/m.gguf", listener.local_addr().unwrap());
    let body = bytes.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(&body);
        }
    });

    let model = ModelDescriptor {
        id: "m".into(),
        display_name: "M".into(),
        description: String::new(),
        version: "v1".into(),
        url,
        file_name: "m.gguf".into(),
        size: bytes.len() as u64,
        sha256: Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        license: License {
            id: "MIT".into(),
            url: String::new(),
        },
        languages: vec![],
        dimensions: 8,
        pooling: "last".into(),
        query_prefix: "Instruct: q\nQuery: ".into(),
        passage_prefix: String::new(),
        context_size: 4096,
        recommended: true,
    };
    let data_dir = std::env::temp_dir().join(format!("nlmx-activation-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    let models = LocalModelProvider::new(
        data_dir.join("models"),
        data_dir.join("embedding.json"),
        vec![model],
    )
    .with_disk_meter(Arc::new(|_: &Path| Ok(u64::MAX / 2)));

    let plan = models.plan_download("m").await.unwrap();
    models
        .download(plan.confirm(), Arc::new(|_| {}), CancelFlag::default())
        .await
        .unwrap();
    let installed = models.activate("m").await.unwrap();

    // The embedding side finds the same file through its default config location.
    let config = EmbeddingConfig::load(&config_path(&data_dir)).unwrap();
    assert_eq!(config.model_id, "m");
    assert_eq!(config.model_path.display().to_string(), installed.path);
    assert_eq!(
        (config.pooling.as_str(), config.context_size),
        ("last", 4096)
    );
    assert_eq!(config.query_prefix, "Instruct: q\nQuery: ");
}
