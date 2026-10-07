//! Records `Measurement`s as structured `tracing` events (target `nlmx::metrics`). The
//! measurement type carries no text, so recording can never leak document content.

use std::time::Instant;

use nlmx_domain::telemetry::Measurement;

pub const TARGET: &str = "nlmx::metrics";

pub fn record(measurement: &Measurement) {
    tracing::info!(
        target: "nlmx::metrics",
        measurement = measurement.name(),
        data = %measurement.to_json(),
    );
}

/// Milliseconds since `start`.
pub fn ms(start: Instant) -> u64 {
    start.elapsed().as_millis() as u64
}
