//! Configurações › Diagnóstico: the session's local measurements (throughput, latency
//! percentiles, error rate), memory and storage. Aggregates only — no document content.

use nlmx_application::ports::{DiagnosticsSnapshot, OperationStats};
use nlmx_domain::telemetry::Operation;
use serde_json::json;

pub struct OperationRow {
    pub label: &'static str,
    pub total: u64,
    pub failed: u64,
    pub error_rate: String,
    pub p50: String,
    pub p95: String,
}

pub struct DiagnosticsView {
    pub uptime: String,
    pub documents: u64,
    pub pages: u64,
    pub chunks: u64,
    pub pages_per_second: String,
    pub embeddings: u64,
    pub embeddings_per_second: String,
    pub first_token: String,
    pub operations: Vec<OperationRow>,
    pub memory: Vec<(&'static str, String)>,
    pub storage: Vec<(&'static str, String)>,
    /// For "Copiar diagnóstico".
    pub json: String,
}

pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit]).replace('.', ",")
    }
}

pub fn duration(ms: Option<u64>) -> String {
    match ms {
        None => "—".into(),
        Some(ms) if ms < 1000 => format!("{ms} ms"),
        Some(ms) => format!("{:.1} s", ms as f64 / 1000.0).replace('.', ","),
    }
}

fn per_second(value: Option<f64>) -> String {
    value.map_or_else(|| "—".into(), |v| format!("{v:.1}/s").replace('.', ","))
}

fn label(op: Operation) -> &'static str {
    match op {
        Operation::Ingest => "Importação",
        Operation::Embed => "Embeddings",
        Operation::Retrieve => "Busca (retrieval)",
        Operation::Generate => "Geração de respostas",
    }
}

fn row(o: &OperationStats) -> OperationRow {
    OperationRow {
        label: label(o.operation),
        total: o.total,
        failed: o.failed,
        error_rate: o.error_rate().map_or_else(
            || "—".into(),
            |r| format!("{:.1}%", r * 100.0).replace('.', ","),
        ),
        p50: duration(o.p50_ms),
        p95: duration(o.p95_ms),
    }
}

impl From<&DiagnosticsSnapshot> for DiagnosticsView {
    fn from(s: &DiagnosticsSnapshot) -> Self {
        let mut memory = Vec::new();
        if let Some(r) = s.resources {
            memory.push(("Aplicativo", bytes(r.rss_bytes)));
            if let Some(b) = r.llama_rss_bytes {
                memory.push(("Servidor de embeddings (llama.cpp)", bytes(b)));
            }
            if let Some(b) = r.fm_rss_bytes {
                memory.push(("Apple Foundation Models (fm)", bytes(b)));
            }
        }
        let mut storage = Vec::new();
        if let Some(st) = s.storage {
            storage.push(("Banco de dados e índices", bytes(st.database_bytes)));
            storage.push(("Biblioteca de PDFs", bytes(st.library_bytes)));
            storage.push(("Modelos", bytes(st.models_bytes)));
            storage.push(("Logs", bytes(st.logs_bytes)));
            storage.push(("Total", bytes(st.total())));
        }
        let minutes = s.uptime_secs / 60;
        Self {
            uptime: if minutes < 1 {
                "menos de 1 min".into()
            } else {
                format!("{minutes} min")
            },
            documents: s.documents_imported,
            pages: s.pages,
            chunks: s.chunks,
            pages_per_second: per_second(s.pages_per_second),
            embeddings: s.embeddings,
            embeddings_per_second: per_second(s.embeddings_per_second),
            first_token: duration(s.first_token_p50_ms),
            operations: s.operations.iter().map(row).collect(),
            memory,
            storage,
            json: serde_json::to_string_pretty(&snapshot_json(s)).unwrap_or_default(),
        }
    }
}

pub fn snapshot_json(s: &DiagnosticsSnapshot) -> serde_json::Value {
    json!({
        "uptime_secs": s.uptime_secs,
        "operations": s.operations.iter().map(|o| json!({
            "operation": o.operation.as_str(),
            "total": o.total,
            "failed": o.failed,
            "error_rate": o.error_rate(),
            "p50_ms": o.p50_ms,
            "p95_ms": o.p95_ms,
            "max_ms": o.max_ms,
        })).collect::<Vec<_>>(),
        "documents_imported": s.documents_imported,
        "pages": s.pages,
        "chunks": s.chunks,
        "pages_per_second": s.pages_per_second,
        "embeddings": s.embeddings,
        "embeddings_per_second": s.embeddings_per_second,
        "first_token_p50_ms": s.first_token_p50_ms,
        "memory_bytes": s.resources.map(|r| json!({
            "app": r.rss_bytes, "llama": r.llama_rss_bytes, "fm": r.fm_rss_bytes,
        })),
        "storage_bytes": s.storage.map(|st| json!({
            "database": st.database_bytes, "library": st.library_bytes,
            "models": st.models_bytes, "logs": st.logs_bytes, "total": st.total(),
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes_and_durations_in_portuguese() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1,5 KB");
        assert_eq!(bytes(5 * 1024 * 1024), "5,0 MB");
        assert_eq!(duration(Some(640)), "640 ms");
        assert_eq!(duration(Some(2100)), "2,1 s");
        assert_eq!(duration(None), "—");
    }
}
