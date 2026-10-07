//! Local telemetry: the metrics registry fed by `nlmx::metrics` events, structured JSON logs
//! with redaction (and size rotation), and memory/storage sampling. Nothing leaves the Mac.

pub mod logs;
pub mod redact;
pub mod registry;
pub mod sampler;

use std::{sync::Arc, time::Duration};

use nlmx_application::{
    ports::{BoxFuture, Diagnostics, DiagnosticsSnapshot},
    telemetry::TARGET,
};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{
    EnvFilter, Layer, layer::Context, layer::SubscriberExt, util::SubscriberInitExt,
};

pub use logs::{ConsoleSink, Format, LOG_FILE, LogLayer, LogSink, MemorySink, RotatingFile};
pub use registry::{MetricsRegistry, snapshot_json};
pub use sampler::DataLayout;

/// Feeds `nlmx::metrics` events into a [`MetricsRegistry`].
pub struct MetricsLayer {
    registry: MetricsRegistry,
}

impl MetricsLayer {
    pub fn new(registry: MetricsRegistry) -> Self {
        Self { registry }
    }
}

#[derive(Default)]
struct MeasurementFields {
    name: Option<String>,
    data: Option<String>,
}

impl Visit for MeasurementFields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "measurement" => self.name = Some(value.into()),
            "data" => self.data = Some(value.into()),
            _ => {}
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // `data = %json` arrives as a Display-formatted debug value.
        self.record_str(field, &format!("{value:?}"));
    }
}

impl<S: Subscriber> Layer<S> for MetricsLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != TARGET {
            return;
        }
        let mut fields = MeasurementFields::default();
        event.record(&mut fields);
        if let (Some(name), Some(data)) = (fields.name, fields.data) {
            self.registry.record(&name, &data);
        }
    }
}

/// The app's diagnostics: registry snapshot + on-demand sampling.
pub struct LocalDiagnostics {
    pub registry: MetricsRegistry,
    pub layout: DataLayout,
}

impl Diagnostics for LocalDiagnostics {
    fn snapshot(&self) -> DiagnosticsSnapshot {
        self.registry.snapshot()
    }

    fn sample(&self) -> BoxFuture<'_, ()> {
        let layout = self.layout.clone();
        Box::pin(async move {
            let _ = tokio::task::spawn_blocking(move || layout.sample()).await;
        })
    }
}

pub struct TelemetryConfig {
    pub layout: DataLayout,
    /// Readable logs on stderr too (development).
    pub console: bool,
    pub max_log_bytes: u64,
    pub keep_logs: usize,
}

impl TelemetryConfig {
    pub fn new(layout: DataLayout) -> Self {
        Self {
            layout,
            console: cfg!(debug_assertions),
            max_log_bytes: 5 * 1024 * 1024,
            keep_logs: 5,
        }
    }
}

/// Installs the global subscriber: JSON logs (filtered by `RUST_LOG`, default `info`), the
/// optional console, and the metrics layer (never filtered). Returns the diagnostics.
pub fn init(config: TelemetryConfig) -> Arc<LocalDiagnostics> {
    let registry = MetricsRegistry::new();
    let filter = || {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info"))
            // Measurements are aggregated, not logged one by one.
            .add_directive(format!("{TARGET}=off").parse().expect("valid directive"))
    };
    let file = LogLayer::new(
        RotatingFile::new(&config.layout.logs, config.max_log_bytes, config.keep_logs),
        Format::Json,
    )
    .with_filter(filter());
    let console = config
        .console
        .then(|| LogLayer::new(ConsoleSink, Format::Text).with_filter(filter()));
    let _ = tracing_subscriber::registry()
        .with(file)
        .with(console)
        .with(MetricsLayer::new(registry.clone()))
        .try_init();
    Arc::new(LocalDiagnostics {
        registry,
        layout: config.layout,
    })
}

/// Samples memory and storage every `period` (spawn it on the app's runtime).
pub async fn run_sampler(diagnostics: Arc<LocalDiagnostics>, period: Duration) {
    loop {
        diagnostics.sample().await;
        tokio::time::sleep(period).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nlmx_domain::telemetry::Measurement;

    #[test]
    fn measurements_reach_the_registry_through_tracing() {
        let registry = MetricsRegistry::new();
        let subscriber = tracing_subscriber::registry().with(MetricsLayer::new(registry.clone()));
        tracing::subscriber::with_default(subscriber, || {
            nlmx_application::telemetry::record(&Measurement::Embedded {
                document_id: 1,
                chunks: 50,
                dims: 1024,
                total_ms: 1000,
            });
            tracing::info!("not a measurement");
        });
        let s = registry.snapshot();
        assert_eq!(s.embeddings, 50);
        assert_eq!(s.embeddings_per_second, Some(50.0));
    }
}
