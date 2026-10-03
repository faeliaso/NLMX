//! `EmbeddingProvider` over the supervised llama-server (`POST /v1/embeddings`).

use std::{sync::Arc, time::Duration};

use nlmx_application::ports::{BoxFuture, EmbeddingProvider};
use nlmx_domain::embedding::{EmbeddingError, EmbeddingPurpose, ModelIdentity};
use serde::Deserialize;
use tokio::sync::{Mutex, OnceCell};

use crate::{
    config::EmbeddingConfig,
    server::{Endpoint, LlamaServer},
};

pub struct LlamaCppEmbeddingProvider {
    config: EmbeddingConfig,
    server: Arc<LlamaServer>,
    http: reqwest::Client,
    request_timeout: Duration,
    identity: OnceCell<ModelIdentity>,
    /// One batch request at a time keeps memory bounded on the server.
    in_flight: Mutex<()>,
}

#[derive(Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    meta: Option<ModelMeta>,
}

#[derive(Deserialize)]
struct ModelMeta {
    n_embd: u32,
    #[serde(default)]
    n_ctx_train: u32,
    #[serde(default)]
    n_params: u64,
}

#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Deserialize)]
struct EmbeddingItem {
    index: usize,
    embedding: Vec<f32>,
}

impl LlamaCppEmbeddingProvider {
    pub fn new(config: EmbeddingConfig, server: Arc<LlamaServer>) -> Self {
        Self {
            config,
            server,
            http: reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("HTTP client"),
            request_timeout: Duration::from_secs(60),
            identity: OnceCell::new(),
            in_flight: Mutex::new(()),
        }
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub fn server(&self) -> &Arc<LlamaServer> {
        &self.server
    }

    /// Starts the server now instead of on the first request.
    pub async fn start(&self) -> Result<(), EmbeddingError> {
        self.server.start().await.map(drop)
    }

    pub async fn shutdown(&self) {
        self.server.stop().await;
    }

    fn prefix(&self, purpose: EmbeddingPurpose) -> &str {
        match purpose {
            EmbeddingPurpose::Query => &self.config.query_prefix,
            EmbeddingPurpose::Passage => &self.config.passage_prefix,
        }
    }

    async fn load_identity(&self) -> Result<ModelIdentity, EmbeddingError> {
        let _use = self.server.begin_use();
        let endpoint = self.server.ensure_running().await?;
        let response = self
            .http
            .get(format!("{}/v1/models", endpoint.base_url))
            .bearer_auth(&endpoint.api_key)
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(request_error)?;
        let models: ModelsResponse = parse(response).await?;
        let meta = models
            .data
            .into_iter()
            .find_map(|m| m.meta)
            .ok_or_else(|| {
                EmbeddingError::InvalidResponse("/v1/models sem metadados do modelo".into())
            })?;
        let path = &self.config.model_path;
        Ok(ModelIdentity {
            id: self.config.model_id.clone(),
            file_name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            file_size: std::fs::metadata(path).map(|m| m.len()).unwrap_or_default(),
            dimensions: meta.n_embd,
            context_length: meta.n_ctx_train,
            parameters: meta.n_params,
        })
    }

    async fn identity_cached(&self) -> Result<&ModelIdentity, EmbeddingError> {
        self.identity.get_or_try_init(|| self.load_identity()).await
    }

    async fn embed_inputs(
        &self,
        texts: &[String],
        purpose: EmbeddingPurpose,
    ) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let dimensions = self.identity_cached().await?.dimensions;
        let prefix = self.prefix(purpose);
        // Held for the whole call, so the server is not stopped for idling between batches.
        let _use = self.server.begin_use();
        let _guard = self.in_flight.lock().await;
        let mut vectors = Vec::with_capacity(texts.len());
        for batch in texts.chunks(self.config.max_batch_inputs) {
            let inputs: Vec<String> = batch.iter().map(|t| format!("{prefix}{t}")).collect();
            let endpoint = self.server.ensure_running().await?;
            let result = match self.request(&endpoint, &inputs).await {
                // The server died (crash, killed): restart once and retry this batch.
                Err(RequestFailure::Connection(reason)) => {
                    tracing::warn!("llama-server unreachable ({reason}); restarting");
                    let endpoint = self.server.restart().await?;
                    self.request(&endpoint, &inputs)
                        .await
                        .map_err(RequestFailure::into_error)
                }
                other => other.map_err(RequestFailure::into_error),
            }?;
            vectors.extend(validate(result, batch.len(), dimensions)?);
        }
        Ok(vectors)
    }

    async fn request(
        &self,
        endpoint: &Endpoint,
        inputs: &[String],
    ) -> Result<EmbeddingsResponse, RequestFailure> {
        let response = self
            .http
            .post(format!("{}/v1/embeddings", endpoint.base_url))
            .bearer_auth(&endpoint.api_key)
            .timeout(self.request_timeout)
            .json(&serde_json::json!({ "input": inputs, "model": self.config.model_id, "encoding_format": "float" }))
            .send()
            .await
            .map_err(|err| {
                if err.is_connect() {
                    RequestFailure::Connection(err.to_string())
                } else {
                    RequestFailure::Other(request_error(err))
                }
            })?;
        parse(response).await.map_err(RequestFailure::Other)
    }
}

enum RequestFailure {
    Connection(String),
    Other(EmbeddingError),
}

impl RequestFailure {
    fn into_error(self) -> EmbeddingError {
        match self {
            Self::Connection(reason) => {
                EmbeddingError::Unavailable(format!("servidor inacessível: {reason}"))
            }
            Self::Other(err) => err,
        }
    }
}

fn request_error(err: reqwest::Error) -> EmbeddingError {
    if err.is_timeout() {
        EmbeddingError::Timeout("o servidor de embeddings não respondeu a tempo".into())
    } else {
        EmbeddingError::Unavailable(err.to_string())
    }
}

async fn parse<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, EmbeddingError> {
    let status = response.status();
    let body = response.text().await.map_err(request_error)?;
    if !status.is_success() {
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
            })
            .unwrap_or(body);
        let lower = message.to_lowercase();
        return Err(
            if lower.contains("too large") || lower.contains("exceed") || lower.contains("context")
            {
                EmbeddingError::InputTooLong(message)
            } else {
                EmbeddingError::Server(format!("{status}: {message}"))
            },
        );
    }
    serde_json::from_str(&body).map_err(|err| EmbeddingError::InvalidResponse(err.to_string()))
}

/// Checks count, order and dimension, and L2-normalizes each vector.
fn validate(
    response: EmbeddingsResponse,
    expected: usize,
    dimensions: u32,
) -> Result<Vec<Vec<f32>>, EmbeddingError> {
    let mut items = response.data;
    if items.len() != expected {
        return Err(EmbeddingError::InvalidResponse(format!(
            "{} vetores para {expected} textos",
            items.len()
        )));
    }
    items.sort_by_key(|item| item.index);
    items
        .into_iter()
        .enumerate()
        .map(|(position, item)| {
            if item.index != position {
                return Err(EmbeddingError::InvalidResponse(format!(
                    "índice {} fora de ordem",
                    item.index
                )));
            }
            if item.embedding.len() != dimensions as usize {
                return Err(EmbeddingError::InvalidResponse(format!(
                    "vetor com {} dimensões (esperado {dimensions})",
                    item.embedding.len()
                )));
            }
            normalize(item.embedding)
        })
        .collect()
}

fn normalize(mut vector: Vec<f32>) -> Result<Vec<f32>, EmbeddingError> {
    let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return Err(EmbeddingError::InvalidResponse(
            "vetor nulo ou inválido".into(),
        ));
    }
    vector.iter_mut().for_each(|x| *x /= norm);
    Ok(vector)
}

impl EmbeddingProvider for LlamaCppEmbeddingProvider {
    fn model_id(&self) -> &str {
        &self.config.model_id
    }

    fn identity(&self) -> BoxFuture<'_, Result<ModelIdentity, EmbeddingError>> {
        Box::pin(async move { self.identity_cached().await.cloned() })
    }

    fn dimensions(&self) -> BoxFuture<'_, Result<u32, EmbeddingError>> {
        Box::pin(async move { Ok(self.identity_cached().await?.dimensions) })
    }

    fn embed<'a>(
        &'a self,
        text: &'a str,
        purpose: EmbeddingPurpose,
    ) -> BoxFuture<'a, Result<Vec<f32>, EmbeddingError>> {
        Box::pin(async move {
            let mut vectors = self.embed_inputs(&[text.to_string()], purpose).await?;
            Ok(vectors.remove(0))
        })
    }

    fn embed_batch<'a>(
        &'a self,
        texts: &'a [String],
        purpose: EmbeddingPurpose,
    ) -> BoxFuture<'a, Result<Vec<Vec<f32>>, EmbeddingError>> {
        Box::pin(self.embed_inputs(texts, purpose))
    }
}
