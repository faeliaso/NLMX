//! Structured logs: JSON lines in a size-rotated file (and readable text on the console in
//! debug), every field passed through the redaction rules.

use std::{
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde_json::{Map, Value};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
    span,
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

use crate::redact::{REDACTED, is_sensitive, sanitize};

/// Where log lines go.
pub trait LogSink: Send + Sync + 'static {
    fn write_line(&self, line: &str);
}

/// Appends to `<dir>/nlmx.jsonl`, rotating to `.1` … `.N` past `max_bytes`.
pub struct RotatingFile {
    dir: PathBuf,
    max_bytes: u64,
    keep: usize,
    file: Mutex<Option<(File, u64)>>,
}

pub const LOG_FILE: &str = "nlmx.jsonl";

impl RotatingFile {
    pub fn new(dir: impl Into<PathBuf>, max_bytes: u64, keep: usize) -> Self {
        Self {
            dir: dir.into(),
            max_bytes,
            keep: keep.max(1),
            file: Mutex::new(None),
        }
    }

    fn path(&self, n: usize) -> PathBuf {
        if n == 0 {
            self.dir.join(LOG_FILE)
        } else {
            self.dir.join(format!("{LOG_FILE}.{n}"))
        }
    }

    fn open(&self) -> Option<(File, u64)> {
        fs::create_dir_all(&self.dir).ok()?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path(0))
            .ok()?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Some((file, size))
    }

    fn rotate(&self) {
        let _ = fs::remove_file(self.path(self.keep));
        for n in (0..self.keep).rev() {
            let _ = fs::rename(self.path(n), self.path(n + 1));
        }
    }
}

impl LogSink for RotatingFile {
    fn write_line(&self, line: &str) {
        let mut slot = self.file.lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|(_, size)| *size >= self.max_bytes)
        {
            *slot = None;
            self.rotate();
        }
        if slot.is_none() {
            *slot = self.open();
        }
        if let Some((file, size)) = slot.as_mut() {
            if writeln!(file, "{line}").is_ok() {
                *size += line.len() as u64 + 1;
            }
        }
    }
}

/// Collects lines in memory (tests).
#[derive(Clone, Default)]
pub struct MemorySink(pub Arc<Mutex<Vec<String>>>);

impl LogSink for MemorySink {
    fn write_line(&self, line: &str) {
        self.0.lock().unwrap().push(line.to_string());
    }
}

/// Writes readable lines to stderr (development).
pub struct ConsoleSink;

impl LogSink for ConsoleSink {
    fn write_line(&self, line: &str) {
        eprintln!("{line}");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Text,
}

/// Field values of an event or span, redacted.
#[derive(Default)]
struct Fields {
    message: Option<String>,
    values: Map<String, Value>,
}

impl Fields {
    fn put(&mut self, field: &Field, value: Value) {
        let name = field.name();
        if is_sensitive(name) {
            self.values
                .insert(name.into(), Value::String(REDACTED.into()));
        } else {
            self.values.insert(name.into(), value);
        }
    }
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let text = sanitize(&format!("{value:?}"));
        if field.name() == "message" {
            self.message = Some(text);
        } else {
            self.put(field, Value::String(text));
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(sanitize(value));
        } else {
            self.put(field, Value::String(sanitize(value)));
        }
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.put(field, Value::from(value));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.put(field, Value::from(value));
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.put(field, Value::from(value));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.put(field, Value::from(value));
    }
}

/// Stored in a span's extensions: its redacted fields.
struct SpanFields(Map<String, Value>);

pub struct LogLayer<K: LogSink> {
    sink: K,
    format: Format,
}

impl<K: LogSink> LogLayer<K> {
    pub fn new(sink: K, format: Format) -> Self {
        Self { sink, format }
    }
}

fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        now.subsec_millis()
    )
}

impl<S, K> Layer<S> for LogLayer<K>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    K: LogSink,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        attrs.record(&mut fields);
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(SpanFields(fields.values));
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let meta = event.metadata();
        let mut spans: Vec<Value> = Vec::new();
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                let mut entry = Map::new();
                entry.insert("name".into(), Value::String(span.name().into()));
                if let Some(f) = span.extensions().get::<SpanFields>() {
                    entry.extend(f.0.clone());
                }
                spans.push(Value::Object(entry));
            }
        }
        let line = match self.format {
            Format::Json => {
                let mut record = Map::new();
                record.insert("ts".into(), Value::String(timestamp()));
                record.insert("level".into(), Value::String(meta.level().to_string()));
                record.insert("target".into(), Value::String(meta.target().into()));
                if let Some(message) = fields.message {
                    record.insert("message".into(), Value::String(message));
                }
                if !fields.values.is_empty() {
                    record.insert("fields".into(), Value::Object(fields.values));
                }
                if !spans.is_empty() {
                    record.insert("spans".into(), Value::Array(spans));
                }
                Value::Object(record).to_string()
            }
            Format::Text => {
                let mut line = format!(
                    "{} {:>5} {}: {}",
                    timestamp(),
                    meta.level(),
                    meta.target(),
                    fields.message.unwrap_or_default()
                );
                for (k, v) in &fields.values {
                    let _ = write!(line, " {k}={v}");
                }
                line
            }
        };
        self.sink.write_line(&line);
    }
}

/// Total size of the log files in `dir`.
pub fn logs_size(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with(LOG_FILE))
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn json_lines_with_redacted_fields_and_masked_paths() {
        let sink = MemorySink::default();
        let subscriber =
            tracing_subscriber::registry().with(LogLayer::new(sink.clone(), Format::Json));
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("ingest", document_id = 7, title = "Laudo secreto");
            let _g = span.enter();
            tracing::warn!(
                pages = 3,
                question = "qual o CPF?",
                file = "/Users/ana/x.pdf",
                "falha ao ler /Users/ana/Docs/x.pdf"
            );
        });
        let lines = sink.0.lock().unwrap();
        assert_eq!(lines.len(), 1);
        let v: Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(v["level"], "WARN");
        assert_eq!(v["message"], "falha ao ler <path>");
        assert_eq!(v["fields"]["pages"], 3);
        assert_eq!(v["fields"]["question"], REDACTED);
        assert_eq!(v["fields"]["file"], REDACTED);
        assert_eq!(v["spans"][0]["name"], "ingest");
        assert_eq!(v["spans"][0]["document_id"], 7);
        assert_eq!(v["spans"][0]["title"], REDACTED);
        assert!(v["ts"].as_str().unwrap().ends_with('Z'));
        assert!(
            !lines[0].contains("secreto") && !lines[0].contains("CPF") && !lines[0].contains("ana")
        );
    }

    #[test]
    fn rotates_by_size() {
        let dir = std::env::temp_dir().join(format!("nlmx-logs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let file = RotatingFile::new(&dir, 100, 2);
        for i in 0..20 {
            file.write_line(&format!("{{\"n\":{i},\"pad\":\"xxxxxxxxxxxxxxxxxxxx\"}}"));
        }
        assert!(dir.join(LOG_FILE).exists());
        assert!(dir.join(format!("{LOG_FILE}.1")).exists());
        assert!(dir.join(format!("{LOG_FILE}.2")).exists());
        assert!(
            !dir.join(format!("{LOG_FILE}.3")).exists(),
            "keeps 2 rotated files"
        );
        assert!(fs::metadata(dir.join(LOG_FILE)).unwrap().len() <= 150);
        assert!(logs_size(&dir) > 0);
    }
}
