//! Aggregates the session's measurements (events with target `nlmx::metrics`): counters,
//! duration percentiles, throughput and error rate per operation, latest memory and storage.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Instant,
};

use nlmx_application::ports::{DiagnosticsSnapshot, OperationStats, ResourceUsage, StorageUsage};
use nlmx_domain::{
    generation::{GenerationUsage, LastGeneration},
    telemetry::Operation,
};
use serde_json::Value;

/// Durations kept per operation (oldest dropped first) for percentiles.
const MAX_SAMPLES: usize = 2000;

#[derive(Default)]
struct OpState {
    total: u64,
    failed: u64,
    durations: Vec<u64>,
}

struct State {
    started: Instant,
    ops: BTreeMap<Operation, OpState>,
    documents: u64,
    pages: u64,
    chunks: u64,
    ingest_ms: u64,
    embeddings: u64,
    embed_ms: u64,
    first_tokens: Vec<u64>,
    /// Latest generation whose tokens were counted; a later one that wasn't (failed, cancelled,
    /// no count) leaves it as it was.
    last_generation: Option<LastGeneration>,
    resources: Option<ResourceUsage>,
    storage: Option<StorageUsage>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            ops: BTreeMap::new(),
            documents: 0,
            pages: 0,
            chunks: 0,
            ingest_ms: 0,
            embeddings: 0,
            embed_ms: 0,
            first_tokens: Vec::new(),
            last_generation: None,
            resources: None,
            storage: None,
        }
    }
}

#[derive(Clone, Default)]
pub struct MetricsRegistry {
    state: Arc<Mutex<State>>,
}

fn push(samples: &mut Vec<u64>, value: u64) {
    if samples.len() == MAX_SAMPLES {
        samples.remove(0);
    }
    samples.push(value);
}

fn percentile(samples: &[u64], p: f64) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    Some(sorted[rank - 1])
}

fn rate(count: u64, ms: u64) -> Option<f64> {
    (count > 0 && ms > 0).then(|| count as f64 * 1000.0 / ms as f64)
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// One measurement as emitted by `application::telemetry::record`.
    pub fn record(&self, name: &str, data: &str) {
        let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(data) else {
            return;
        };
        let u = |key: &str| fields.get(key).and_then(Value::as_u64);
        let total_ms = u("total_ms").unwrap_or(0);
        let outcome = match name {
            "ingested" => Some((Operation::Ingest, false)),
            "ingest_failed" => Some((Operation::Ingest, true)),
            "embedded" => Some((Operation::Embed, false)),
            "embed_failed" => Some((Operation::Embed, true)),
            "retrieved" => Some((Operation::Retrieve, false)),
            "retrieve_failed" => Some((Operation::Retrieve, true)),
            "generated" => Some((Operation::Generate, false)),
            "generation_failed" => Some((Operation::Generate, true)),
            _ => None,
        };
        let mut s = self.state.lock().unwrap();
        if let Some((op, failed)) = outcome {
            let entry = s.ops.entry(op).or_default();
            entry.total += 1;
            entry.failed += u64::from(failed);
            push(&mut entry.durations, total_ms);
        }
        match name {
            "ingested" => {
                s.documents += 1;
                s.pages += u("pages").unwrap_or(0);
                s.chunks += u("chunks").unwrap_or(0);
                // Throughput over the extraction work, not the embedding that may follow.
                s.ingest_ms += u("extract_ms").unwrap_or(0)
                    + u("structure_ms").unwrap_or(0)
                    + u("chunk_ms").unwrap_or(0)
                    + u("save_ms").unwrap_or(0);
            }
            "embedded" => {
                s.embeddings += u("chunks").unwrap_or(0);
                s.embed_ms += total_ms;
            }
            "generated" => {
                if let Some(ms) = u("first_token_ms") {
                    push(&mut s.first_tokens, ms);
                }
                let count = |key: &str| u(key).and_then(|n| u32::try_from(n).ok());
                if let (Some(turn), Some(prompt_tokens), Some(completion_tokens)) = (
                    count("turn"),
                    count("reported_prompt_tokens"),
                    count("completion_tokens"),
                ) {
                    s.last_generation = Some(LastGeneration {
                        turn,
                        usage: GenerationUsage {
                            prompt_tokens,
                            completion_tokens,
                        },
                    });
                }
            }
            "resources" => {
                s.resources = Some(ResourceUsage {
                    rss_bytes: u("rss_bytes").unwrap_or(0),
                    llama_rss_bytes: u("llama_rss_bytes"),
                    fm_rss_bytes: u("fm_rss_bytes"),
                });
            }
            "storage" => {
                s.storage = Some(StorageUsage {
                    database_bytes: u("database_bytes").unwrap_or(0),
                    library_bytes: u("library_bytes").unwrap_or(0),
                    models_bytes: u("models_bytes").unwrap_or(0),
                    logs_bytes: u("logs_bytes").unwrap_or(0),
                });
            }
            _ => {}
        }
    }

    pub fn snapshot(&self) -> DiagnosticsSnapshot {
        let s = self.state.lock().unwrap();
        DiagnosticsSnapshot {
            uptime_secs: s.started.elapsed().as_secs(),
            operations: Operation::ALL
                .into_iter()
                .map(|op| {
                    let o = s.ops.get(&op);
                    let d = o.map(|o| o.durations.as_slice()).unwrap_or_default();
                    OperationStats {
                        operation: op,
                        total: o.map_or(0, |o| o.total),
                        failed: o.map_or(0, |o| o.failed),
                        p50_ms: percentile(d, 0.5),
                        p95_ms: percentile(d, 0.95),
                        max_ms: d.iter().copied().max(),
                    }
                })
                .collect(),
            documents_imported: s.documents,
            pages: s.pages,
            chunks: s.chunks,
            pages_per_second: rate(s.pages, s.ingest_ms),
            embeddings: s.embeddings,
            embeddings_per_second: rate(s.embeddings, s.embed_ms),
            first_token_p50_ms: percentile(&s.first_tokens, 0.5),
            last_generation: s.last_generation,
            resources: s.resources,
            storage: s.storage,
        }
    }
}

/// JSON for "Copiar diagnóstico" and benchmark reports (aggregates only).
pub fn snapshot_json(s: &DiagnosticsSnapshot) -> Value {
    let ops: Vec<Value> = s
        .operations
        .iter()
        .map(|o| {
            serde_json::json!({
                "operation": o.operation.as_str(),
                "total": o.total,
                "failed": o.failed,
                "error_rate": o.error_rate(),
                "p50_ms": o.p50_ms,
                "p95_ms": o.p95_ms,
                "max_ms": o.max_ms,
            })
        })
        .collect();
    serde_json::json!({
        "uptime_secs": s.uptime_secs,
        "operations": ops,
        "documents_imported": s.documents_imported,
        "pages": s.pages,
        "chunks": s.chunks,
        "pages_per_second": s.pages_per_second,
        "embeddings": s.embeddings,
        "embeddings_per_second": s.embeddings_per_second,
        "first_token_p50_ms": s.first_token_p50_ms,
        "memory": s.resources.map(|r| serde_json::json!({
            "rss_bytes": r.rss_bytes,
            "llama_rss_bytes": r.llama_rss_bytes,
            "fm_rss_bytes": r.fm_rss_bytes,
        })),
        "storage": s.storage.map(|st| serde_json::json!({
            "database_bytes": st.database_bytes,
            "library_bytes": st.library_bytes,
            "models_bytes": st.models_bytes,
            "logs_bytes": st.logs_bytes,
            "total_bytes": st.total(),
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nlmx_domain::telemetry::{ErrorKind, IngestStage, Measurement};

    fn feed(r: &MetricsRegistry, m: Measurement) {
        r.record(m.name(), &m.to_json());
    }

    #[test]
    fn aggregates_throughput_percentiles_and_error_rate() {
        let r = MetricsRegistry::new();
        for (pages, ms) in [(10, 500), (30, 1500)] {
            feed(
                &r,
                Measurement::Ingested {
                    document_id: 1,
                    bytes: 100,
                    pages,
                    chunks: pages * 2,
                    extract_ms: ms,
                    structure_ms: 0,
                    chunk_ms: 0,
                    save_ms: 0,
                    total_ms: ms + 10,
                },
            );
        }
        feed(
            &r,
            Measurement::IngestFailed {
                document_id: 2,
                stage: IngestStage::Extract,
                kind: ErrorKind::Corrupt,
                total_ms: 5,
            },
        );
        feed(
            &r,
            Measurement::Embedded {
                document_id: 1,
                chunks: 80,
                dims: 1024,
                total_ms: 4000,
            },
        );
        for (i, ms) in [100u64, 200, 300, 400].into_iter().enumerate() {
            feed(
                &r,
                Measurement::Generated {
                    intent: "regular",
                    status: "answered",
                    prompt_tokens: 500,
                    first_token_ms: Some(50 * (i as u64 + 1)),
                    output_chars: 100,
                    total_ms: ms,
                    turn: 1,
                    reported_prompt_tokens: None,
                    completion_tokens: None,
                },
            );
        }
        feed(
            &r,
            Measurement::Resources {
                rss_bytes: 1 << 20,
                llama_rss_bytes: Some(2 << 20),
                fm_rss_bytes: None,
            },
        );
        let s = r.snapshot();
        assert_eq!((s.documents_imported, s.pages, s.chunks), (2, 40, 80));
        assert_eq!(s.pages_per_second, Some(20.0));
        assert_eq!(s.embeddings_per_second, Some(20.0));
        let ingest = s.operation(Operation::Ingest).unwrap();
        assert_eq!((ingest.total, ingest.failed), (3, 1));
        assert!((ingest.error_rate().unwrap() - 1.0 / 3.0).abs() < 1e-9);
        let generate = s.operation(Operation::Generate).unwrap();
        assert_eq!(
            (generate.p50_ms, generate.p95_ms, generate.max_ms),
            (Some(200), Some(400), Some(400))
        );
        assert_eq!(s.first_token_p50_ms, Some(100));
        assert_eq!(s.operation(Operation::Retrieve).unwrap().error_rate(), None);
        assert_eq!(s.resources.unwrap().llama_rss_bytes, Some(2 << 20));
        let json = snapshot_json(&s).to_string();
        assert!(json.contains("\"error_rate\"") && json.contains("\"pages_per_second\":20.0"));
    }

    fn generated(turn: u32, usage: Option<(u32, u32)>) -> Measurement {
        Measurement::Generated {
            intent: "free",
            status: "answered",
            prompt_tokens: 10,
            first_token_ms: Some(40),
            output_chars: 100,
            total_ms: 900,
            turn,
            reported_prompt_tokens: usage.map(|(prompt, _)| prompt),
            completion_tokens: usage.map(|(_, completion)| completion),
        }
    }

    fn last(r: &MetricsRegistry) -> Option<(u32, u32, u32)> {
        r.snapshot()
            .last_generation
            .map(|g| (g.turn, g.usage.prompt_tokens, g.usage.completion_tokens))
    }

    #[test]
    fn keeps_the_tokens_of_the_latest_counted_generation() {
        let r = MetricsRegistry::new();
        assert_eq!(last(&r), None);
        feed(&r, generated(1, Some((101, 19))));
        assert_eq!(last(&r), Some((1, 101, 19)));
        feed(&r, generated(2, Some((1655, 343))));
        assert_eq!(last(&r), Some((2, 1655, 343)));
        // Cancelled or uncounted, and failed generations leave the last counted one in place.
        feed(&r, generated(3, None));
        feed(
            &r,
            Measurement::GenerationFailed {
                kind: ErrorKind::Unavailable,
                total_ms: 3,
            },
        );
        assert_eq!(last(&r), Some((2, 1655, 343)));
    }

    #[test]
    fn ignores_malformed_events() {
        let r = MetricsRegistry::new();
        r.record("ingested", "not json");
        r.record("unknown", "{}");
        assert_eq!(r.snapshot().documents_imported, 0);
    }
}
