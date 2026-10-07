//! Configurações › Diagnóstico: the session's local measurements (throughput, latency
//! percentiles, error rate), memory and storage. Aggregates only — no document content.

use nlmx_application::ports::{DiagnosticsSnapshot, OperationStats};
use nlmx_domain::telemetry::Operation;
use nlmx_i18n::{Arg, Locale, format_bytes, format_number, tr, tr_args};
use serde_json::json;

pub struct OperationRow {
    pub label: String,
    pub total: u64,
    pub failed: u64,
    pub error_rate: String,
    pub p50: String,
    pub p95: String,
}

pub struct DiagnosticsView {
    /// "Measurements from this session (3 min)…".
    pub help: String,
    /// "3 · 40 pages · 200 passages".
    pub documents_summary: String,
    pub pages_per_second: String,
    /// "1,5/s (200 in total)".
    pub embeddings_rate: String,
    pub first_token: String,
    pub operations: Vec<OperationRow>,
    pub memory: Vec<(String, String)>,
    pub storage: Vec<(String, String)>,
    /// For "Copiar diagnóstico".
    pub json: String,
}

fn bytes(locale: Locale, n: u64) -> String {
    format_bytes(locale, n)
}

fn duration(locale: Locale, ms: Option<u64>) -> String {
    match ms {
        None => "—".into(),
        Some(ms) if ms < 1000 => format!("{ms} ms"),
        Some(ms) => format!("{} s", format_number(locale, ms as f64 / 1000.0, 1)),
    }
}

fn per_second(locale: Locale, value: Option<f64>) -> String {
    value.map_or_else(
        || "—".into(),
        |v| format!("{}/s", format_number(locale, v, 1)),
    )
}

/// A 0..=1 ratio with one decimal (`12,5%`, `12.5%`, `12,5 %` in Spanish).
fn error_rate(locale: Locale, ratio: f64) -> String {
    let n = format_number(locale, ratio * 100.0, 1);
    match locale {
        Locale::Es => format!("{n} %"),
        _ => format!("{n}%"),
    }
}

fn label(locale: Locale, op: Operation) -> String {
    tr(
        locale,
        match op {
            Operation::Ingest => "diagnostics-op-ingest",
            Operation::Embed => "diagnostics-op-embed",
            Operation::Retrieve => "diagnostics-op-retrieve",
            Operation::Generate => "diagnostics-op-generate",
        },
    )
}

fn row(locale: Locale, o: &OperationStats) -> OperationRow {
    OperationRow {
        label: label(locale, o.operation),
        total: o.total,
        failed: o.failed,
        error_rate: o
            .error_rate()
            .map_or_else(|| "—".into(), |r| error_rate(locale, r)),
        p50: duration(locale, o.p50_ms),
        p95: duration(locale, o.p95_ms),
    }
}

impl From<&DiagnosticsSnapshot> for DiagnosticsView {
    fn from(s: &DiagnosticsSnapshot) -> Self {
        Self::in_locale(nlmx_i18n::current(), s)
    }
}

impl DiagnosticsView {
    pub fn in_locale(locale: Locale, s: &DiagnosticsSnapshot) -> Self {
        let mut memory = Vec::new();
        if let Some(r) = s.resources {
            memory.push((
                tr(locale, "diagnostics-mem-app"),
                bytes(locale, r.rss_bytes),
            ));
            if let Some(b) = r.llama_rss_bytes {
                memory.push((tr(locale, "diagnostics-mem-llama"), bytes(locale, b)));
            }
            if let Some(b) = r.fm_rss_bytes {
                memory.push((tr(locale, "diagnostics-mem-fm"), bytes(locale, b)));
            }
        }
        let mut storage = Vec::new();
        if let Some(st) = s.storage {
            for (id, value) in [
                ("diagnostics-storage-database", st.database_bytes),
                ("diagnostics-storage-library", st.library_bytes),
                ("diagnostics-storage-models", st.models_bytes),
                ("diagnostics-storage-logs", st.logs_bytes),
                ("diagnostics-storage-total", st.total()),
            ] {
                storage.push((tr(locale, id), bytes(locale, value)));
            }
        }
        let minutes = s.uptime_secs / 60;
        let uptime = if minutes < 1 {
            tr(locale, "diagnostics-uptime-short")
        } else {
            tr_args(
                locale,
                "diagnostics-uptime-minutes",
                &[("count", Arg::from(minutes))],
            )
        };
        Self {
            help: tr_args(locale, "diagnostics-help", &[("uptime", uptime.into())]),
            documents_summary: tr_args(
                locale,
                "diagnostics-documents-summary",
                &[
                    ("documents", s.documents_imported.into()),
                    ("pages", s.pages.into()),
                    ("chunks", s.chunks.into()),
                ],
            ),
            pages_per_second: per_second(locale, s.pages_per_second),
            embeddings_rate: tr_args(
                locale,
                "diagnostics-embeddings-rate",
                &[
                    ("rate", per_second(locale, s.embeddings_per_second).into()),
                    ("total", s.embeddings.into()),
                ],
            ),
            first_token: duration(locale, s.first_token_p50_ms),
            operations: s.operations.iter().map(|o| row(locale, o)).collect(),
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
    fn rates_and_sizes_follow_the_language() {
        assert_eq!(error_rate(Locale::PtBr, 0.125), "12,5%");
        assert_eq!(error_rate(Locale::En, 0.125), "12.5%");
        assert_eq!(error_rate(Locale::Es, 0.125), "12,5 %");
        assert_eq!(duration(Locale::PtBr, Some(1500)), "1,5 s");
        assert_eq!(duration(Locale::En, Some(1500)), "1.5 s");
        assert_eq!(per_second(Locale::En, Some(2.0)), "2.0/s");
        assert_eq!(duration(Locale::En, None), "—");
    }

    #[test]
    fn labels_are_localized() {
        assert_eq!(label(Locale::PtBr, Operation::Ingest), "Importação");
        assert_eq!(label(Locale::En, Operation::Generate), "Answer generation");
    }
}
