//! Composition root: the only place where concrete adapters are instantiated and injected
//! into use cases.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

use nlmx_application::{
    ports::{
        BoxFuture, CancelFlag, DocumentEngine, EmbeddingProvider, EmbeddingSource, ModelProvider,
        ProgressSink, StorageDiagnostics, StorageError, StorageInfo,
    },
    services::{
        free_chat::FreeChat,
        parsing::{ParserRegistry, PdfDocumentParser},
        pipeline::ContentPipeline,
        rag::{RagEngine, RagOptions},
        retrieval::HybridRetriever,
        retriever::Retriever,
    },
    use_cases::{
        ChatService, DocumentIngestion, EmbedDocuments, GetSystemStatus, Indexing,
        IndexingActivity, RemoveDocument, ViewDocument,
    },
};
use nlmx_chunker_structural::{HeuristicTokenCounter, MultiFormatChunker};
use nlmx_domain::{
    ingestion::{ChunkPolicy, DocumentId, IngestProgress},
    models::ModelState,
};
use nlmx_embed_llama::{EmbeddingConfig, LlamaCppEmbeddingProvider, LlamaCppRuntime};
use nlmx_fs_library::{FsLibrary, LIBRARY_DIR};
use nlmx_llm_fm::FoundationModelsProvider;
use nlmx_models_catalog::LocalModelProvider;
use nlmx_normalizer_text::TextNormalizer;
use nlmx_parser_epub::EpubDocumentParser;
use nlmx_parser_office::{DocxDocumentParser, XlsxDocumentParser};
use nlmx_parser_text::{
    CsvDocumentParser, MarkdownDocumentParser, NoteDocumentParser, TextDocumentParser,
};
use nlmx_pdf_pdfium::{PDFIUM_BUILD, PdfiumDocumentEngine};
use nlmx_store_sqlite::{DATABASE_FILE, Database};
use nlmx_structure_heuristic::HeuristicStructureAnalyzer;
use nlmx_ui_web::AppState;

/// The UI router, stored in Tauri's managed state for the custom-protocol handler.
pub struct UiRouter(pub axum::Router);

/// The ingestion use case for Tauri commands; `Err` carries why it is unavailable.
pub struct Ingestion(pub Result<Arc<DocumentIngestion>, String>);

/// The progress of the documents being indexed (see [`ProgressRelay`]).
pub struct IngestProgressState(pub Arc<ProgressRelay>);

/// Forwards the pipeline's progress to the interface and remembers what is still running, so a
/// screen opened midway can show it. The emitter is attached once the app handle exists.
type ProgressEmitter = Box<dyn Fn(&IngestProgress) + Send + Sync>;

#[derive(Default)]
pub struct ProgressRelay {
    emit: std::sync::OnceLock<ProgressEmitter>,
    active: Mutex<HashMap<DocumentId, IngestProgress>>,
}

impl ProgressRelay {
    pub fn attach(&self, emit: impl Fn(&IngestProgress) + Send + Sync + 'static) {
        let _ = self.emit.set(Box::new(emit));
    }

    /// The documents being processed right now, by id.
    pub fn snapshot(&self) -> Vec<IngestProgress> {
        let mut active: Vec<_> = self.active.lock().unwrap().values().cloned().collect();
        active.sort_by_key(|p| p.document_id);
        active
    }
}

impl ProgressSink for ProgressRelay {
    fn report(&self, progress: IngestProgress) {
        let finished = matches!(progress.phase, "indexed" | "failed" | "waiting")
            || progress.status == "needs_ocr";
        {
            let mut active = self.active.lock().unwrap();
            if finished {
                active.remove(&progress.document_id);
            } else {
                active.insert(progress.document_id, progress.clone());
            }
        }
        if let Some(emit) = self.emit.get() {
            emit(&progress);
        }
    }
}

/// The current embedding model. Replaced when a model is downloaded, activated or removed;
/// its llama-server starts on first use and is stopped when the model is replaced.
pub struct EmbeddingSlot {
    data_dir: PathBuf,
    current: RwLock<Option<Arc<LlamaCppEmbeddingProvider>>>,
}

impl EmbeddingSlot {
    fn load(data_dir: &Path) -> Arc<Self> {
        Arc::new(Self {
            data_dir: data_dir.to_path_buf(),
            current: RwLock::new(embedding_provider(data_dir)),
        })
    }

    pub fn llama(&self) -> Option<Arc<LlamaCppEmbeddingProvider>> {
        self.current.read().unwrap().clone()
    }

    /// Re-reads `embedding.json`; returns the previous provider (to be shut down) and whether the
    /// model changed (vectors of the old model are not comparable with the new one).
    pub fn reload(&self) -> (Option<Arc<LlamaCppEmbeddingProvider>>, bool) {
        let next = embedding_provider(&self.data_dir);
        let mut current = self.current.write().unwrap();
        let old_id = current.as_ref().map(|p| p.model_id().to_string());
        let new_id = next.as_ref().map(|p| p.model_id().to_string());
        let previous = std::mem::replace(&mut *current, next);
        (previous, old_id.is_some() && old_id != new_id)
    }
}

impl EmbeddingSource for EmbeddingSlot {
    fn current(&self) -> Option<Arc<dyn EmbeddingProvider>> {
        self.llama().map(|p| p as Arc<dyn EmbeddingProvider>)
    }
}

/// Model files (catalog, downloads, activation). Downloads only run after user confirmation.
pub struct Models {
    pub provider: Arc<LocalModelProvider>,
    /// Cancel flags of downloads in progress, by model id.
    pub downloads: Mutex<HashMap<String, CancelFlag>>,
}

/// Embeds stored chunks with the current model (`None` without a database).
pub struct Embedder(pub Option<Arc<EmbedDocuments>>);

/// The Indexação actions (`Err`: why they are unavailable) and the background work counter,
/// shared with imports, the startup resume and model changes.
pub struct IndexingState {
    pub indexing: Result<Arc<Indexing>, String>,
    pub activity: Arc<IndexingActivity>,
}

/// Tells the UI that background indexing work ended (`app.js` relays it to HTMX).
pub fn notify_indexing_changed<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    use tauri::Emitter;
    let _ = app.emit("indexing-changed", ());
}

/// The Chat: answers generated with Apple Foundation Models, streamed by `answer_message`.
pub struct Chat {
    pub service: Result<Arc<ChatService>, String>,
    /// Cancel flags of answers being generated, by message id.
    pub running: Mutex<HashMap<i64, CancelFlag>>,
    /// Kept to stop `fm serve` when the app quits.
    pub llm: Arc<FoundationModelsProvider>,
}

/// Everything the shell keeps in managed state.
pub struct Services {
    pub ui: UiRouter,
    pub ingestion: Ingestion,
    pub progress: Arc<ProgressRelay>,
    pub embeddings: Arc<EmbeddingSlot>,
    pub embedder: Embedder,
    pub indexing: IndexingState,
    pub models: Models,
    pub chat: Chat,
    pub diagnostics: DiagnosticsState,
    /// Also cleans the library of orphan files at startup.
    pub remover: Result<Arc<RemoveDocument>, String>,
    /// Where the UI hands pasted notes to the import queue (connected once the queue starts).
    pub notes: Arc<crate::importer::NoteInbox>,
}

/// Builds the app from its data directory (`~/Library/Application Support/<identifier>`).
/// A database or PDF engine that cannot be loaded does not stop the app: the UI starts and shows the error.
/// Where the app keeps its data, for storage and memory sampling.
pub fn data_layout(data_dir: &Path) -> nlmx_telemetry::DataLayout {
    nlmx_telemetry::DataLayout {
        database: data_dir.join(DATABASE_FILE),
        library: data_dir.join(LIBRARY_DIR),
        models: data_dir.join(nlmx_models_catalog::MODELS_DIR),
        logs: data_dir.join("logs"),
        run: data_dir.join("run"),
    }
}

/// Session diagnostics (absent in tests that build without telemetry).
pub struct DiagnosticsState(pub Option<Arc<dyn nlmx_application::ports::Diagnostics>>);

impl DiagnosticsState {
    /// Samples memory and storage in the background (after imports and model changes).
    pub fn sample_later(&self) {
        if let Some(d) = self.0.clone() {
            tauri::async_runtime::spawn(async move { d.sample().await });
        }
    }
}

pub fn build(
    data_dir: &Path,
    diagnostics: Option<Arc<nlmx_telemetry::LocalDiagnostics>>,
) -> Services {
    let diagnostics: Option<Arc<dyn nlmx_application::ports::Diagnostics>> =
        diagnostics.map(|d| d as Arc<dyn nlmx_application::ports::Diagnostics>);
    let database = open_database(data_dir);
    let storage: Arc<dyn StorageDiagnostics> = match &database {
        Ok(db) => {
            tracing::info!(database = DATABASE_FILE, "database ready");
            Arc::new(db.clone())
        }
        Err(err) => {
            tracing::error!("database unavailable: {err}");
            Arc::new(UnavailableStorage(err.clone()))
        }
    };

    let engine = PdfiumDocumentEngine::from_default_location();
    let engine_status = match &engine {
        Ok(_) => {
            tracing::info!(build = PDFIUM_BUILD, "PDF engine ready");
            Ok(PDFIUM_BUILD.to_string())
        }
        Err(err) => {
            tracing::error!("PDF engine unavailable: {err}");
            Err(err.to_string())
        }
    };

    let progress = Arc::new(ProgressRelay::default());
    let embeddings = EmbeddingSlot::load(data_dir);
    let db = database.as_ref().ok().map(|db| Arc::new(db.clone()));
    let embedder = db.as_ref().map(|db| {
        Arc::new(EmbedDocuments {
            progress: Some(progress.clone() as Arc<dyn ProgressSink>),
            embeddings: embeddings.clone(),
            vectors: db.clone(),
            chunks: db.clone(),
            documents: db.clone(),
            batch_size: 32,
        })
    });
    let retriever = match &db {
        Some(db) => Ok(Arc::new(Retriever::new(Arc::new(HybridRetriever::new(
            embeddings.clone(),
            db.clone(),
            db.clone(),
            db.clone(),
        ))))),
        None => Err("Database unavailable".to_string()),
    };

    let engine: Result<Arc<dyn DocumentEngine>, String> = engine
        .map(|e| Arc::new(e) as Arc<dyn DocumentEngine>)
        .map_err(|e| e.to_string());
    let viewer = match (&db, &engine) {
        (Some(db), Ok(engine)) => Ok(Arc::new(ViewDocument::new(engine.clone(), db.clone()))),
        (None, _) => Err("Database unavailable".to_string()),
        (_, Err(err)) => Err(format!("PDF engine unavailable: {err}")),
    };
    let library = Arc::new(FsLibrary::new(data_dir.join(LIBRARY_DIR)));
    let remover = match &db {
        Some(db) => Ok(Arc::new(RemoveDocument {
            documents: db.clone(),
            files: library.clone(),
            viewer: viewer.as_ref().ok().cloned(),
        })),
        None => Err("Database unavailable".to_string()),
    };
    let ingestion = match (database, engine) {
        (Ok(db), Ok(engine)) => {
            // One parser per format: the registry picks it by the file's type, nothing else
            // in the pipeline knows the formats.
            let parsers = ParserRegistry::new()
                .with(Arc::new(PdfDocumentParser::new(
                    engine.clone(),
                    Arc::new(HeuristicStructureAnalyzer),
                )))
                .with(Arc::new(MarkdownDocumentParser))
                .with(Arc::new(TextDocumentParser))
                .with(Arc::new(CsvDocumentParser))
                .with(Arc::new(EpubDocumentParser::default()))
                .with(Arc::new(DocxDocumentParser::default()))
                .with(Arc::new(XlsxDocumentParser::default()))
                .with(Arc::new(NoteDocumentParser::new()));
            let tokens = Arc::new(HeuristicTokenCounter);
            Ok(Arc::new(DocumentIngestion {
                pipeline: Some(Arc::new(ContentPipeline {
                    parsers,
                    normalizer: Arc::new(TextNormalizer),
                    chunker: Arc::new(MultiFormatChunker),
                    tokens: tokens.clone(),
                    policy: ChunkPolicy::default(),
                })),
                progress: Some(progress.clone() as Arc<dyn ProgressSink>),
                viewer: viewer.as_ref().ok().cloned(),
                engine,
                files: library,
                documents: Arc::new(db),
                analyzer: Arc::new(HeuristicStructureAnalyzer),
                chunker: Arc::new(nlmx_chunker_structural::StructuralChunker),
                tokens,
                policy: ChunkPolicy::default(),
                embedder: embedder.clone(),
            }))
        }
        (Err(err), _) => Err(format!("Database unavailable: {err}")),
        (_, Err(err)) => Err(format!("PDF engine unavailable: {err}")),
    };

    let activity = Arc::new(IndexingActivity::default());
    let indexing = match (&db, &embedder) {
        (Some(db), Some(embedder)) => Ok(Arc::new(Indexing {
            reader: db.clone(),
            documents: db.clone(),
            embeddings: embeddings.clone(),
            embedder: embedder.clone(),
            ingestion: ingestion.clone(),
            activity: activity.clone(),
        })),
        _ => Err("Database unavailable".to_string()),
    };

    let models = Arc::new(LocalModelProvider::in_data_dir(data_dir));
    let language_model = Arc::new(system_fm(data_dir));
    let chat = Chat {
        service: match (&retriever, &db) {
            (Ok(r), Some(db)) => Ok(Arc::new(ChatService {
                conversations: db.clone(),
                rag: Arc::new(RagEngine::new(
                    r.clone(),
                    db.clone(),
                    language_model.clone(),
                )),
                free: Arc::new(FreeChat::new(language_model.clone())),
                options: RagOptions::default(),
            })),
            (Err(e), _) => Err(e.clone()),
            (_, None) => Err("Database unavailable".to_string()),
        },
        running: Mutex::new(HashMap::new()),
        llm: language_model.clone(),
    };
    let notes = Arc::new(crate::importer::NoteInbox::default());
    let ui = UiRouter(nlmx_ui_web::router(AppState {
        system_status: Arc::new(GetSystemStatus::new(
            language_model,
            storage,
            engine_status,
            Arc::new(LlamaCppRuntime::detect()),
        )),
        ingestion: ingestion.clone(),
        chat: chat.service.clone(),
        viewer,
        remover: remover.clone(),
        diagnostics: diagnostics.clone(),
        models: Some(models.clone() as Arc<dyn ModelProvider>),
        indexing: indexing.clone(),
        notes: Some(notes.clone()),
        language: nlmx_ui_web::LanguageSettings::new(
            db.clone()
                .map(|db| db as Arc<dyn nlmx_application::ports::SettingsRepository>),
            sys_locale::get_locales().collect(),
        ),
    }));
    Services {
        ui,
        ingestion: Ingestion(ingestion),
        progress,
        embeddings,
        embedder: Embedder(embedder),
        indexing: IndexingState { indexing, activity },
        models: Models {
            provider: models,
            downloads: Mutex::new(HashMap::new()),
        },
        chat,
        diagnostics: DiagnosticsState(diagnostics),
        remover,
        notes,
    }
}

/// Reads the external model configuration; missing or invalid ⇒ no provider (logged).
fn embedding_provider(data_dir: &Path) -> Option<Arc<LlamaCppEmbeddingProvider>> {
    let path = nlmx_embed_llama::config_path(data_dir);
    if !path.exists() {
        tracing::info!("no embedding model configured");
        return None;
    }
    let config = match EmbeddingConfig::load(&path) {
        Ok(config) => config,
        Err(err) => {
            tracing::error!("invalid embedding configuration: {err}");
            return None;
        }
    };
    let Some(binary) = nlmx_embed_llama::llama_server_path() else {
        tracing::error!("llama-server not found (run `make bootstrap` or set NLMX_LLAMA_SERVER)");
        return None;
    };
    tracing::info!(model = %config.model_id, "embedding provider configured");
    Some(Arc::new(nlmx_embed_llama::provider(
        config, binary, data_dir,
    )))
}

/// After a model change: stop the old llama-server and embed what is pending (everything, if the
/// model changed, since vectors of different models are not comparable). `on_done` runs when the
/// embeddings are updated.
pub async fn apply_model_change(
    slot: &EmbeddingSlot,
    embedder: Option<Arc<EmbedDocuments>>,
    activity: Arc<IndexingActivity>,
    on_done: impl FnOnce() + Send + 'static,
) {
    let (previous, changed) = slot.reload();
    if let Some(previous) = previous {
        previous.shutdown().await;
    }
    if let Some(embedder) = embedder {
        let running = activity.begin();
        tauri::async_runtime::spawn(async move {
            let outcomes = if changed {
                embedder.reindex_all().await
            } else {
                embedder.embed_pending().await
            };
            tracing::info!(
                documents = outcomes.len(),
                "embeddings updated after model change"
            );
            drop(running);
            on_done();
        });
    }
}

/// Logs the state of each catalog model at startup: corrupted files and available updates.
pub async fn report_models(models: &dyn ModelProvider) {
    for model in models.catalog() {
        match models.status(&model.id).await {
            Ok(ModelState::Corrupted { reason }) => {
                tracing::warn!(model = %model.id, "model corrupted: {reason}")
            }
            Ok(ModelState::UpdateAvailable { installed, latest }) => {
                tracing::info!(model = %model.id, %installed, %latest, "model update available")
            }
            Ok(ModelState::Installed { version }) => {
                tracing::info!(model = %model.id, %version, "model installed")
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(model = %model.id, "model status unavailable: {err}"),
        }
    }
}

fn open_database(data_dir: &Path) -> Result<Database, StorageError> {
    std::fs::create_dir_all(data_dir).map_err(|err| {
        StorageError::new(format!(
            "Could not create the data folder {}: {err}",
            data_dir.display()
        ))
    })?;
    Database::open(data_dir.join(DATABASE_FILE))
}

/// Stands in for the database when it failed to open, so the failure is visible in the UI.
struct UnavailableStorage(StorageError);

impl StorageDiagnostics for UnavailableStorage {
    fn info(&self) -> BoxFuture<'_, Result<StorageInfo, StorageError>> {
        let err = self.0.clone();
        Box::pin(async move { Err(err) })
    }
}

/// The system `fm`. Debug builds accept `NLMX_FM_BINARY=<script>` to simulate its states
/// (e.g. a script that exits 69 shows the license dialog).
fn system_fm(data_dir: &Path) -> FoundationModelsProvider {
    #[cfg(debug_assertions)]
    if let Ok(binary) = std::env::var("NLMX_FM_BINARY") {
        return FoundationModelsProvider::new(nlmx_llm_fm::FoundationModelsConfig::new(
            binary,
            data_dir.join("run"),
        ));
    }
    FoundationModelsProvider::system(data_dir.join("run"))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use http::Request;
    use tower::ServiceExt;

    use super::*;

    async fn settings_page(router: axum::Router) -> String {
        let response = router
            .oneshot(Request::get("/settings").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[tokio::test]
    async fn creates_and_migrates_the_database_in_the_data_dir() {
        let dir = std::env::temp_dir().join(format!("nlmx-wiring-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let Services {
            ui: UiRouter(router),
            ..
        } = build(&dir, None);
        assert!(dir.join(DATABASE_FILE).exists());
        let page = settings_page(router).await;
        assert!(page.contains(DATABASE_FILE));
        let latest = nlmx_store_sqlite::LATEST_VERSION.to_string();
        // Worded in whatever language this machine resolved to ("N de N", "N of N"…).
        assert!(page.contains(&nlmx_i18n::t_args(
            "status-schema-version",
            &[
                ("current", latest.as_str().into()),
                ("latest", latest.as_str().into())
            ],
        )));
    }

    #[tokio::test]
    async fn an_unusable_data_dir_still_starts_the_ui_with_the_error() {
        let Services {
            ui: UiRouter(router),
            ..
        } = build(Path::new("/dev/null/nlmx"), None);
        let page = settings_page(router).await;
        // The heading follows whatever language this machine resolved to.
        assert!(page.contains(&nlmx_i18n::t("settings-database-unavailable")));
    }
}
