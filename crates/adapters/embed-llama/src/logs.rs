//! Server output: appended to a log file (with simple rotation), mirrored to `tracing`, and kept
//! in a small in-memory buffer for error messages and diagnostics.

use std::{
    collections::VecDeque,
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::Mutex,
};

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

const KEEP_LINES: usize = 200;
const ROTATE_BYTES: u64 = 5 * 1024 * 1024;

pub struct LogSink {
    path: PathBuf,
    file: Mutex<Option<File>>,
    recent: Mutex<VecDeque<String>>,
}

impl LogSink {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            file: Mutex::new(None),
            recent: Mutex::new(VecDeque::new()),
        }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn recent(&self) -> Vec<String> {
        self.recent.lock().unwrap().iter().cloned().collect()
    }

    /// The last `n` lines joined, for error messages.
    pub fn tail(&self, n: usize) -> String {
        let recent = self.recent.lock().unwrap();
        let skip = recent.len().saturating_sub(n);
        recent
            .iter()
            .skip(skip)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn write(&self, line: &str) {
        {
            let mut recent = self.recent.lock().unwrap();
            if recent.len() == KEEP_LINES {
                recent.pop_front();
            }
            recent.push_back(line.to_string());
        }
        if line.contains("error") || line.contains("failed") {
            tracing::warn!(target: "llama_server", "{line}");
        } else {
            tracing::debug!(target: "llama_server", "{line}");
        }
        let mut file = self.file.lock().unwrap();
        if file.is_none() || std::fs::metadata(&self.path).is_ok_and(|m| m.len() > ROTATE_BYTES) {
            *file = self.open_rotating();
        }
        if let Some(f) = file.as_mut() {
            let _ = writeln!(f, "{line}");
        }
    }

    fn open_rotating(&self) -> Option<File> {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() > ROTATE_BYTES) {
            let _ = std::fs::rename(&self.path, self.path.with_extension("log.1"));
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .ok()
    }

    /// Forwards every line of a child pipe into the sink until it closes.
    pub fn follow(self: &std::sync::Arc<Self>, pipe: impl AsyncRead + Unpin + Send + 'static) {
        let sink = std::sync::Arc::clone(self);
        tokio::spawn(async move {
            let mut lines = BufReader::new(pipe).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                sink.write(&line);
            }
        });
    }
}
