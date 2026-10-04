//! The import queue: picking files returns at once and the work happens here, one file at a
//! time, so the user can keep importing while earlier files are processed.
//!
//! Each file is registered first (`DocumentIngestion::enqueue`: it shows in the library as
//! `queued`) and then processed. The interface is told when the library changes
//! (`documents-changed`) and, when a batch is done, gets one summary (`import-finished`).

use std::{path::PathBuf, sync::Arc};

use nlmx_application::use_cases::{ActivityGuard, DocumentIngestion, Enqueued, IndexingActivity};
use nlmx_domain::ingestion::ImportOutcome;
use serde::Serialize;
use tauri::{Emitter, Manager, Runtime};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

/// What the import work tells the outside world. Implemented over the Tauri app; tests record.
pub trait Notifier: Send + Sync + 'static {
    /// The library changed (a document appeared or moved to another status).
    fn documents_changed(&self);
    /// A batch of files is done: `kind` is `success` or `danger`.
    fn finished(&self, kind: &'static str, message: String);
}

/// The payload of the `import-finished` event.
#[derive(Debug, Clone, Serialize)]
pub struct ImportFinished {
    pub kind: &'static str,
    pub message: String,
}

/// The one-line outcome of a batch: "2 importados · 1 já estava na biblioteca — falha".
pub fn summarize(
    imported: usize,
    duplicates: usize,
    failures: &[String],
) -> (&'static str, String) {
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut parts = Vec::new();
    if imported > 0 {
        parts.push(plural(imported, "importado", "importados"));
    }
    if duplicates > 0 {
        parts.push(plural(
            duplicates,
            "já estava na biblioteca",
            "já estavam na biblioteca",
        ));
    }
    if !failures.is_empty() {
        parts.push(plural(failures.len(), "com falha", "com falha"));
    }
    let mut message = parts.join(" · ");
    if let Some(first) = failures.first() {
        message.push_str(&format!(" — {first}"));
    }
    (
        if failures.is_empty() {
            "success"
        } else {
            "danger"
        },
        message,
    )
}

/// The files of one import after they were registered: each with its name and what
/// registering it produced.
pub struct Batch {
    items: Vec<(String, Enqueued)>,
}

/// Registers every file of `paths` in the library at once (status `queued`), telling the
/// interface after each one: they all show in the list right away, whatever else is being
/// processed. Fast (hash and copy); the slow part is [`process_batch`].
pub async fn register_batch(
    ingestion: &Arc<DocumentIngestion>,
    paths: Vec<PathBuf>,
    notify: &Arc<dyn Notifier>,
) -> Batch {
    let mut items = Vec::with_capacity(paths.len());
    for path in paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // One task per file: a panic only fails that file.
        let task = {
            let (ingestion, path) = (ingestion.clone(), path.clone());
            tauri::async_runtime::spawn(async move { ingestion.enqueue(&path).await })
        };
        let enqueued = task.await.unwrap_or_else(|_| Enqueued::Failed {
            id: None,
            reason: "falha interna ao registrar o arquivo".to_string(),
        });
        notify.documents_changed();
        items.push((name, enqueued));
    }
    Batch { items }
}

/// Processes the registered files in order, one at a time. A file that fails (even one that
/// panics a parser) does not stop the others. Returns the summary of the batch.
pub async fn process_batch(
    ingestion: &Arc<DocumentIngestion>,
    batch: Batch,
    notify: &Arc<dyn Notifier>,
) -> (&'static str, String) {
    let (mut imported, mut duplicates, mut failures) = (0, 0, Vec::new());
    for (name, enqueued) in batch.items {
        let task = {
            let ingestion = ingestion.clone();
            tauri::async_runtime::spawn(async move { ingestion.process(enqueued).await })
        };
        let outcome = task.await.unwrap_or_else(|_| ImportOutcome::Failed {
            id: None,
            reason: "falha interna ao processar o arquivo".to_string(),
        });
        match outcome {
            ImportOutcome::Imported { .. } => imported += 1,
            ImportOutcome::Duplicate { .. } => duplicates += 1,
            ImportOutcome::Failed { reason, .. } => {
                // The reason is shown to the user; logs get it without paths (redaction layer).
                tracing::warn!(%reason, "import failed");
                failures.push(format!("{name}: {reason}"));
            }
        }
        notify.documents_changed();
    }
    summarize(imported, duplicates, &failures)
}

/// A registered batch waiting for the worker; the guard keeps the app counted as indexing from
/// the moment the files were picked until the batch is done.
struct Waiting {
    batch: Batch,
    _running: ActivityGuard,
}

/// Imports: files are registered the moment they are picked, processed one at a time.
pub struct ImportQueue {
    ingestion: Arc<DocumentIngestion>,
    activity: Arc<IndexingActivity>,
    notify: Arc<dyn Notifier>,
    tx: UnboundedSender<Waiting>,
}

impl ImportQueue {
    /// Starts the worker. While a batch is registering or waiting the app counts as indexing
    /// (`activity`), so the exclusive actions of Indexação stay disabled.
    pub fn start(
        ingestion: Arc<DocumentIngestion>,
        activity: Arc<IndexingActivity>,
        notify: Arc<dyn Notifier>,
    ) -> Self {
        let (tx, mut rx) = unbounded_channel::<Waiting>();
        {
            let (ingestion, notify) = (ingestion.clone(), notify.clone());
            tauri::async_runtime::spawn(async move {
                while let Some(Waiting { batch, _running }) = rx.recv().await {
                    let (kind, message) = process_batch(&ingestion, batch, &notify).await;
                    drop(_running);
                    notify.finished(kind, message);
                }
            });
        }
        Self {
            ingestion,
            activity,
            notify,
            tx,
        }
    }

    /// Registers the files now (in parallel to whatever is being processed) and queues them
    /// behind the batches already waiting. Returns false if the worker is gone.
    pub fn push(&self, paths: Vec<PathBuf>) -> bool {
        if self.tx.is_closed() {
            return false;
        }
        let running = self.activity.begin();
        let (ingestion, notify, tx) =
            (self.ingestion.clone(), self.notify.clone(), self.tx.clone());
        tauri::async_runtime::spawn(async move {
            let batch = register_batch(&ingestion, paths, &notify).await;
            let _ = tx.send(Waiting {
                batch,
                _running: running,
            });
        });
        true
    }
}

/// The queue in Tauri's managed state (`None`: importing is unavailable).
pub struct ImportQueueState(pub Option<ImportQueue>);

/// Tells the interface through Tauri events.
pub struct TauriNotifier<R: Runtime>(pub tauri::AppHandle<R>);

impl<R: Runtime> Notifier for TauriNotifier<R> {
    fn documents_changed(&self) {
        let _ = self.0.emit("documents-changed", ());
    }

    fn finished(&self, kind: &'static str, message: String) {
        if let Some(diagnostics) = self.0.try_state::<crate::wiring::DiagnosticsState>() {
            diagnostics.sample_later();
        }
        let _ = self
            .0
            .emit("import-finished", ImportFinished { kind, message });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use nlmx_domain::{
        document::DocumentMetadata,
        ingestion::{ChunkPolicy, DocumentStatus},
    };
    use nlmx_testing::{
        FakeChunker, FakeDocumentEngine, FakeDocumentRepository, FakeFileStore, FakePage,
        FakeStructureAnalyzer, WordTokenCounter,
    };

    use super::*;

    #[derive(Default)]
    struct Recorder {
        changes: Mutex<u32>,
        finished: Mutex<Vec<(&'static str, String)>>,
    }

    impl Notifier for Recorder {
        fn documents_changed(&self) {
            *self.changes.lock().unwrap() += 1;
        }

        fn finished(&self, kind: &'static str, message: String) {
            self.finished.lock().unwrap().push((kind, message));
        }
    }

    #[test]
    fn summaries_read_naturally() {
        assert_eq!(summarize(1, 0, &[]), ("success", "1 importado".to_string()));
        assert_eq!(
            summarize(3, 2, &[]),
            (
                "success",
                "3 importados · 2 já estavam na biblioteca".to_string()
            )
        );
        assert_eq!(
            summarize(0, 1, &[]),
            ("success", "1 já estava na biblioteca".to_string())
        );
        let failures = ["a.pdf: PDF inválido".to_string(), "b.md: vazio".to_string()];
        assert_eq!(
            summarize(1, 0, &failures),
            (
                "danger",
                "1 importado · 2 com falha — a.pdf: PDF inválido".to_string()
            )
        );
    }

    fn ingestion(files: &[(&str, &[u8])]) -> (Arc<DocumentIngestion>, Arc<FakeDocumentRepository>) {
        let store = Arc::new(FakeFileStore::default());
        let mut engine = FakeDocumentEngine::default();
        for (path, content) in files {
            let sha = store.add(path, content);
            engine = engine.with_document(
                FakeFileStore::library_path(&sha),
                DocumentMetadata::default(),
                vec![FakePage {
                    spans: vec![],
                    images: vec![],
                }],
            );
        }
        let documents = Arc::new(FakeDocumentRepository::default());
        let ingestion = Arc::new(DocumentIngestion {
            pipeline: None,
            progress: None,
            viewer: None,
            engine: Arc::new(engine),
            files: store,
            documents: documents.clone(),
            analyzer: Arc::new(FakeStructureAnalyzer),
            chunker: Arc::new(FakeChunker),
            tokens: Arc::new(WordTokenCounter),
            policy: ChunkPolicy::default(),
            embedder: None,
        });
        (ingestion, documents)
    }

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|n| PathBuf::from(format!("/in/{n}")))
            .collect()
    }

    #[tokio::test]
    async fn every_file_is_in_the_library_before_any_is_processed() {
        let (ingestion, documents) = ingestion(&[
            ("/in/a.pdf", b"a"),
            ("/in/b.pdf", b"b"),
            ("/in/c.pdf", b"c"),
            ("/in/d.pdf", b"d"),
            ("/in/e.pdf", b"e"),
        ]);
        let recorder = Arc::new(Recorder::default());
        let notify: Arc<dyn Notifier> = recorder.clone();

        let first = register_batch(&ingestion, paths(&["a.pdf", "b.pdf", "c.pdf"]), &notify).await;
        assert_eq!(documents.len(), 3);
        // A second import while the first has not been processed: also in the library at once.
        let second = register_batch(&ingestion, paths(&["d.pdf", "e.pdf"]), &notify).await;
        assert_eq!(documents.len(), 5);
        for id in 1..=5 {
            assert_eq!(
                documents.row(id).status,
                DocumentStatus::Queued,
                "document {id}"
            );
        }
        assert_eq!(
            *recorder.changes.lock().unwrap(),
            5,
            "told once per registered file"
        );

        let (kind, message) = process_batch(&ingestion, first, &notify).await;
        assert_eq!((kind, message.as_str()), ("success", "3 importados"));
        assert_eq!(
            documents.row(4).status,
            DocumentStatus::Queued,
            "the second waits its turn"
        );
        process_batch(&ingestion, second, &notify).await;
        assert!((1..=5).all(|id| documents.row(id).status != DocumentStatus::Queued));
    }

    #[tokio::test]
    async fn duplicates_and_failures_are_decided_when_registering_and_counted_in_the_summary() {
        let (ingestion, documents) = ingestion(&[("/in/a.pdf", b"a")]);
        let notify: Arc<dyn Notifier> = Arc::new(Recorder::default());
        let batch = register_batch(
            &ingestion,
            paths(&["a.pdf", "ausente.pdf", "a.pdf"]),
            &notify,
        )
        .await;
        assert_eq!(documents.len(), 1, "only the first copy is registered");
        let (kind, message) = process_batch(&ingestion, batch, &notify).await;
        assert_eq!(kind, "danger");
        assert!(
            message
                .starts_with("1 importado · 1 já estava na biblioteca · 1 com falha — ausente.pdf"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn batches_are_visible_at_once_and_each_one_reports_once() {
        let (ingestion, documents) = ingestion(&[("/in/a.pdf", b"a"), ("/in/b.pdf", b"b")]);
        let recorder = Arc::new(Recorder::default());
        let activity = Arc::new(IndexingActivity::default());
        let queue = ImportQueue::start(ingestion, activity.clone(), recorder.clone());
        assert!(queue.push(paths(&["a.pdf"])));
        assert!(queue.push(paths(&["b.pdf"])));
        assert!(
            activity.is_running(),
            "counted as indexing from the moment of the push"
        );
        for _ in 0..200 {
            if recorder.finished.lock().unwrap().len() == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let finished = recorder.finished.lock().unwrap().clone();
        assert_eq!(
            finished,
            [
                ("success", "1 importado".to_string()),
                ("success", "1 importado".to_string())
            ]
        );
        assert_eq!(documents.len(), 2);
        assert!(!activity.is_running(), "the activity ends with the queue");
    }
}
