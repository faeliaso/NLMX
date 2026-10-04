//! Tauri commands: native operations the HTML UI cannot do over the custom protocol.
//! Errors are serialized as `{ code, message }` so `app.js` can show them.

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};

#[derive(Debug, Serialize)]
pub struct CommandError {
    code: &'static str,
    message: String,
}

impl CommandError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AppInfo {
    name: String,
    version: String,
    data_dir: String,
}

#[tauri::command]
pub fn app_info<R: Runtime>(app: AppHandle<R>) -> Result<AppInfo, CommandError> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|err| CommandError::new("data_dir", err.to_string()))?;
    Ok(AppInfo {
        name: app.package_info().name.clone(),
        version: app.package_info().version.to_string(),
        data_dir: data_dir.display().to_string(),
    })
}

/// Reveals the app data directory (library, indexes, models) in Finder, creating it if needed.
#[tauri::command]
pub fn open_data_dir<R: Runtime>(app: AppHandle<R>) -> Result<(), CommandError> {
    reveal(&data_dir(&app)?)
}

/// Reveals the logs folder (`<data>/logs`: structured app logs and the llama-server log).
#[tauri::command]
pub fn open_logs_dir<R: Runtime>(app: AppHandle<R>) -> Result<(), CommandError> {
    reveal(&data_dir(&app)?.join("logs"))
}

/// Opens the download page of new versions in the browser (set at build time with
/// `NLMX_DOWNLOAD_URL`; there is no automatic update check).
#[tauri::command]
pub fn open_download_page() -> Result<(), CommandError> {
    let url = nlmx_ui_web::DOWNLOAD_URL
        .filter(|u| u.starts_with("https://"))
        .ok_or_else(|| CommandError::new("open", "Nenhuma página de downloads configurada."))?;
    open(url)
}

fn data_dir<R: Runtime>(app: &AppHandle<R>) -> Result<std::path::PathBuf, CommandError> {
    app.path()
        .app_data_dir()
        .map_err(|err| CommandError::new("data_dir", err.to_string()))
}

fn reveal(dir: &std::path::Path) -> Result<(), CommandError> {
    std::fs::create_dir_all(dir).map_err(|err| {
        CommandError::new("data_dir", format!("Não foi possível criar a pasta: {err}"))
    })?;
    open(dir)
}

fn open(target: impl AsRef<std::ffi::OsStr>) -> Result<(), CommandError> {
    std::process::Command::new("/usr/bin/open")
        .arg(target)
        .status()
        .map_err(|err| CommandError::new("open", format!("Não foi possível abrir: {err}")))
        .and_then(|status| {
            status
                .success()
                .then_some(())
                .ok_or_else(|| CommandError::new("open", "O macOS não abriu o destino."))
        })
}

/// Front-end errors are logged natively so they are not lost inside the WebView.
#[tauri::command]
pub fn report_client_error(message: String, source: String) {
    tracing::error!(target: "ui", %source, "{message}");
}

/// Result shown by the UI as a toast; `refresh` names a DOM event that reloads affected fragments.
#[derive(Debug, Serialize)]
pub struct CommandOutcome {
    kind: &'static str,
    message: String,
    refresh: Option<&'static str>,
}

/// The documents being indexed right now, for a screen opened while they run.
#[tauri::command]
pub fn ingest_progress(
    state: tauri::State<'_, crate::wiring::IngestProgressState>,
) -> Vec<nlmx_domain::ingestion::IngestProgress> {
    state.0.snapshot()
}

/// Picks documents (any supported format) with the native dialog and queues them. It answers as
/// soon as the files are chosen: they are imported one at a time in the background (see
/// `importer`), appear in the library as they are registered and a summary arrives when done.
#[tauri::command]
pub async fn import_documents<R: Runtime>(
    app: AppHandle<R>,
) -> Result<CommandOutcome, CommandError> {
    use tauri_plugin_dialog::DialogExt;

    let ingestion = app
        .state::<crate::wiring::Ingestion>()
        .0
        .clone()
        .map_err(|reason| CommandError::new("unavailable", reason))?;

    // The dialog callback runs on the main thread; hand the selection to this async command.
    let (tx, rx) = tokio::sync::oneshot::channel();
    // The formats come from the parsers that are registered, never from a fixed list here.
    let kinds = ingestion.supported_types();
    let extensions: Vec<&str> = kinds
        .iter()
        .flat_map(|kind| kind.extensions().iter().copied())
        .collect();
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Importar documentos")
        .add_filter("Documentos", &extensions);
    for kind in &kinds {
        dialog = dialog.add_filter(kind.display_name(), kind.extensions());
    }
    dialog.pick_files(move |picked| {
        let _ = tx.send(picked);
    });
    // Cancelling the dialog is not worth a message.
    let Some(picked) = rx.await.ok().flatten() else {
        return Ok(CommandOutcome {
            kind: "info",
            message: String::new(),
            refresh: None,
        });
    };

    let paths: Vec<std::path::PathBuf> = picked
        .into_iter()
        .filter_map(|file| file.into_path().ok())
        .collect();
    if paths.is_empty() {
        return Err(CommandError::new(
            "invalid",
            "Não foi possível ler o caminho dos arquivos escolhidos.",
        ));
    }
    let count = paths.len();
    let queued = app
        .state::<crate::importer::ImportQueueState>()
        .0
        .as_ref()
        .is_some_and(|queue| queue.push(paths));
    if !queued {
        return Err(CommandError::new(
            "unavailable",
            "A fila de importação não está disponível.",
        ));
    }
    Ok(CommandOutcome {
        kind: "info",
        message: match count {
            1 => "Importando 1 arquivo…".to_string(),
            n => format!("Importando {n} arquivos…"),
        },
        refresh: Some("documents-changed"),
    })
}

// ── Indexing ─────────────────────────────────────────────────────────────────

const INDEXING_CHANGED: Option<&str> = Some("indexing-changed");

/// Starts an Indexação action in the background, refused while other indexing work runs. The
/// page refreshes now (showing the work) and again when it ends (`indexing-changed` event).
fn start_indexing<R, F, Fut>(
    app: &AppHandle<R>,
    message: &str,
    action: F,
) -> Result<CommandOutcome, CommandError>
where
    R: Runtime,
    F: FnOnce(std::sync::Arc<nlmx_application::use_cases::Indexing>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let indexing = app
        .state::<crate::wiring::IndexingState>()
        .indexing
        .clone()
        .map_err(|reason| CommandError::new("unavailable", reason))?;
    let running = indexing
        .begin()
        .map_err(|err| CommandError::new("busy", err.to_string()))?;
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        action(indexing).await;
        drop(running);
        crate::wiring::notify_indexing_changed(&handle);
        handle
            .state::<crate::wiring::DiagnosticsState>()
            .sample_later();
    });
    Ok(CommandOutcome {
        kind: "info",
        message: message.into(),
        refresh: INDEXING_CHANGED,
    })
}

/// Reads a failed document again, or embeds again one whose embeddings failed.
#[tauri::command]
pub fn retry_document<R: Runtime>(
    app: AppHandle<R>,
    id: i64,
) -> Result<CommandOutcome, CommandError> {
    start_indexing(
        &app,
        "Processando o documento novamente…",
        move |indexing| async move {
            match indexing.retry(id).await {
                Ok(outcome) => tracing::info!(document_id = id, ?outcome, "document retried"),
                Err(err) => tracing::warn!(document_id = id, "retry failed: {err}"),
            }
        },
    )
}

#[tauri::command]
pub fn retry_failed<R: Runtime>(app: AppHandle<R>) -> Result<CommandOutcome, CommandError> {
    start_indexing(
        &app,
        "Processando novamente os documentos com falha…",
        |indexing| async move {
            match indexing.retry_failed().await {
                Ok(outcomes) => tracing::info!(count = outcomes.len(), "failed documents retried"),
                Err(err) => tracing::warn!("retrying failed documents stopped: {err}"),
            }
        },
    )
}

#[tauri::command]
pub fn embed_pending_now<R: Runtime>(app: AppHandle<R>) -> Result<CommandOutcome, CommandError> {
    start_indexing(
        &app,
        "Gerando os embeddings pendentes…",
        |indexing| async move {
            let outcomes = indexing.embed_pending().await;
            tracing::info!(count = outcomes.len(), "pending documents embedded");
        },
    )
}

/// Embeds every indexed document again with the active model (confirmed in a dialog).
#[tauri::command]
pub fn reindex_all<R: Runtime>(app: AppHandle<R>) -> Result<CommandOutcome, CommandError> {
    start_indexing(&app, "Reindexação iniciada.", |indexing| async move {
        match indexing.reindex_all().await {
            Ok(outcomes) => tracing::info!(count = outcomes.len(), "documents reindexed"),
            Err(err) => tracing::warn!("reindexing not started: {err}"),
        }
    })
}

// ── Models ───────────────────────────────────────────────────────────────────

const MODELS_CHANGED: Option<&str> = Some("models-changed");

#[derive(Debug, Clone, Serialize)]
struct DownloadProgressEvent {
    id: String,
    received: u64,
    total: u64,
    bytes_per_second: f64,
}

fn model_error(err: nlmx_domain::models::ModelError) -> CommandError {
    CommandError::new("model", err.to_string())
}

fn model_name(models: &nlmx_models_catalog::LocalModelProvider, id: &str) -> String {
    use nlmx_application::ports::ModelProvider;
    models
        .catalog()
        .into_iter()
        .find(|m| m.id == id)
        .map(|m| m.display_name)
        .unwrap_or_else(|| id.to_string())
}

async fn after_model_change<R: Runtime>(app: &AppHandle<R>) {
    let slot = app
        .state::<std::sync::Arc<crate::wiring::EmbeddingSlot>>()
        .inner()
        .clone();
    let embedder = app.state::<crate::wiring::Embedder>().0.clone();
    let activity = app.state::<crate::wiring::IndexingState>().activity.clone();
    let handle = app.clone();
    crate::wiring::apply_model_change(&slot, embedder, activity, move || {
        crate::wiring::notify_indexing_changed(&handle)
    })
    .await;
}

/// Downloads a model after the user confirmed its plan in the download dialog.
#[tauri::command]
pub async fn download_model<R: Runtime>(
    app: AppHandle<R>,
    id: String,
) -> Result<CommandOutcome, CommandError> {
    use nlmx_application::ports::{CancelFlag, ModelProvider};
    use nlmx_domain::models::{ModelError, ModelState};
    use tauri::Emitter;

    let state = app.state::<crate::wiring::Models>();
    let models = state.provider.clone();
    let plan = match models.status(&id).await.map_err(model_error)? {
        ModelState::UpdateAvailable { .. } => models.plan_update(&id).await,
        _ => models.plan_download(&id).await,
    }
    .map_err(model_error)?;

    let cancel = CancelFlag::default();
    state
        .downloads
        .lock()
        .unwrap()
        .insert(id.clone(), cancel.clone());
    let emitter = app.clone();
    let event_id = id.clone();
    let progress: nlmx_application::ports::ProgressCallback = std::sync::Arc::new(move |p| {
        let _ = emitter.emit(
            "model-download-progress",
            DownloadProgressEvent {
                id: event_id.clone(),
                received: p.received,
                total: p.total,
                bytes_per_second: p.bytes_per_second,
            },
        );
    });
    // The user confirmed this plan by clicking "Baixar" in the dialog.
    let result = models.download(plan.confirm(), progress, cancel).await;
    state.downloads.lock().unwrap().remove(&id);

    let name = model_name(&models, &id);
    match result {
        Ok(_) => {
            // The first installed model becomes the active one.
            if models.active().await.map_err(model_error)?.is_none() {
                models.activate(&id).await.map_err(model_error)?;
            }
            after_model_change(&app).await;
            Ok(CommandOutcome {
                kind: "success",
                message: format!("{name} instalado e verificado."),
                refresh: MODELS_CHANGED,
            })
        }
        Err(ModelError::Cancelled) => Ok(CommandOutcome {
            kind: "info",
            message: "Download cancelado. Ele pode ser retomado depois.".into(),
            refresh: MODELS_CHANGED,
        }),
        Err(err) => Err(model_error(err)),
    }
}

#[tauri::command]
pub fn cancel_download<R: Runtime>(
    app: AppHandle<R>,
    id: String,
) -> Result<CommandOutcome, CommandError> {
    let state = app.state::<crate::wiring::Models>();
    match state.downloads.lock().unwrap().get(&id) {
        Some(flag) => {
            flag.cancel();
            Ok(CommandOutcome {
                kind: "info",
                message: "Cancelando o download…".into(),
                refresh: None,
            })
        }
        None => Err(CommandError::new(
            "model",
            "Nenhum download em andamento para este modelo.",
        )),
    }
}

#[tauri::command]
pub async fn activate_model<R: Runtime>(
    app: AppHandle<R>,
    id: String,
) -> Result<CommandOutcome, CommandError> {
    use nlmx_application::ports::ModelProvider;
    let models = app.state::<crate::wiring::Models>().provider.clone();
    models.activate(&id).await.map_err(model_error)?;
    after_model_change(&app).await;
    Ok(CommandOutcome {
        kind: "success",
        message: format!(
            "{} em uso. Os documentos serão reindexados em segundo plano.",
            model_name(&models, &id)
        ),
        refresh: MODELS_CHANGED,
    })
}

#[tauri::command]
pub async fn remove_model<R: Runtime>(
    app: AppHandle<R>,
    id: String,
) -> Result<CommandOutcome, CommandError> {
    use nlmx_application::ports::ModelProvider;
    let models = app.state::<crate::wiring::Models>().provider.clone();
    let active = models
        .active()
        .await
        .map_err(model_error)?
        .is_some_and(|m| m.id == id);
    if active {
        // Stop llama-server before deleting the file it has open.
        if let Some(provider) = app
            .state::<std::sync::Arc<crate::wiring::EmbeddingSlot>>()
            .llama()
        {
            provider.shutdown().await;
        }
    }
    models.remove(&id).await.map_err(model_error)?;
    if active {
        after_model_change(&app).await;
    }
    Ok(CommandOutcome {
        kind: "success",
        message: format!("{} removido.", model_name(&models, &id)),
        refresh: MODELS_CHANGED,
    })
}

#[tauri::command]
pub async fn verify_model<R: Runtime>(
    app: AppHandle<R>,
    id: String,
) -> Result<CommandOutcome, CommandError> {
    use nlmx_application::ports::ModelProvider;
    let models = app.state::<crate::wiring::Models>().provider.clone();
    let report = models.verify(&id).await.map_err(model_error)?;
    let name = model_name(&models, &id);
    Ok(if report.ok {
        CommandOutcome {
            kind: "success",
            message: format!("{name}: arquivo íntegro (SHA-256 conferido)."),
            refresh: MODELS_CHANGED,
        }
    } else {
        CommandOutcome {
            kind: "danger",
            message: format!("{name}: o arquivo está corrompido. Remova e baixe novamente."),
            refresh: MODELS_CHANGED,
        }
    })
}

/// What `answer_message` sends through its Channel.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnswerEvent {
    /// A piece of the answer text, as generated.
    Token { text: String },
    /// The answer is saved; fetch `/chat/messages/{id}` for the final rendering.
    Done { status: String },
}

/// Generates the answer of a pending chat message, streaming its text through `on_event`.
/// Only one generation per message: a second call while it runs returns `running`.
#[tauri::command]
pub async fn answer_message<R: Runtime>(
    app: AppHandle<R>,
    message_id: i64,
    on_event: tauri::ipc::Channel<AnswerEvent>,
) -> Result<(), CommandError> {
    let chat = app.state::<crate::wiring::Chat>();
    run_answer(&chat, message_id, &move |event| {
        let _ = on_event.send(event);
    })
    .await
}

/// The logic of `answer_message`, independent of Tauri (tested with fakes).
pub(crate) async fn run_answer(
    chat: &crate::wiring::Chat,
    message_id: i64,
    emit: &(dyn Fn(AnswerEvent) + Send + Sync),
) -> Result<(), CommandError> {
    let service = chat
        .service
        .clone()
        .map_err(|reason| CommandError::new("chat", reason))?;
    let cancel = {
        let mut running = chat.running.lock().unwrap();
        if running.contains_key(&message_id) {
            return Err(CommandError::new(
                "running",
                "Esta resposta já está sendo gerada.",
            ));
        }
        let flag = nlmx_application::ports::CancelFlag::default();
        running.insert(message_id, flag.clone());
        flag
    };
    let on_token = |text: &str| {
        emit(AnswerEvent::Token {
            text: text.to_string(),
        })
    };
    let result = service.answer(message_id, &on_token, cancel).await;
    chat.running.lock().unwrap().remove(&message_id);
    match result {
        Ok(message) => {
            emit(AnswerEvent::Done {
                status: format!("{:?}", message.status).to_lowercase(),
            });
            Ok(())
        }
        Err(nlmx_application::use_cases::ChatError::NotPending) => {
            // Already answered (e.g. by another window): the UI just reloads the fragment.
            emit(AnswerEvent::Done {
                status: "final".into(),
            });
            Ok(())
        }
        Err(err) => Err(CommandError::new("chat", err.to_string())),
    }
}

/// Stops an answer being generated; the partial text is kept.
#[tauri::command]
pub fn cancel_answer<R: Runtime>(app: AppHandle<R>, message_id: i64) -> Result<(), CommandError> {
    run_cancel(&app.state::<crate::wiring::Chat>(), message_id)
}

pub(crate) fn run_cancel(chat: &crate::wiring::Chat, message_id: i64) -> Result<(), CommandError> {
    match chat.running.lock().unwrap().get(&message_id) {
        Some(flag) => {
            flag.cancel();
            Ok(())
        }
        None => Err(CommandError::new(
            "chat",
            "Esta resposta não está sendo gerada.",
        )),
    }
}

#[cfg(test)]
mod tests {
    //! IPC: the command logic with fakes, and a round trip through Tauri's mock runtime.

    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use nlmx_application::{
        services::{
            rag::{RagEngine, RagOptions},
            retrieval::HybridRetriever,
            retriever::{Retriever, RetrieverOptions},
        },
        use_cases::ChatService,
    };
    use nlmx_domain::chat::MessageStatus;
    use nlmx_llm_fm::{FoundationModelsConfig, FoundationModelsProvider};
    use nlmx_testing::{
        FakeConversations, FakeCorpus, FakeLlmProvider, FakeVectorStore, FixedEmbeddingSource,
    };

    use super::*;
    use crate::wiring::Chat;

    fn chat(llm: FakeLlmProvider) -> Arc<Chat> {
        let corpus = Arc::new(FakeCorpus::default().chunk(
            1,
            1,
            2,
            "A carência do plano é de 180 dias para internações.",
        ));
        let hybrid = HybridRetriever::new(
            FixedEmbeddingSource::none(),
            Arc::new(FakeVectorStore::default()),
            corpus.clone(),
            corpus.clone(),
        );
        let llm = Arc::new(llm);
        let service = ChatService {
            conversations: Arc::new(FakeConversations::default()),
            rag: Arc::new(RagEngine::new(
                Arc::new(Retriever::new(Arc::new(hybrid))),
                corpus,
                llm.clone(),
            )),
            free: Arc::new(nlmx_application::services::free_chat::FreeChat::new(llm)),
            options: RagOptions {
                retriever: RetrieverOptions {
                    min_score: 0.0,
                    ..Default::default()
                },
                min_relevance: 0.1,
                ..Default::default()
            },
        };
        Arc::new(Chat {
            service: Ok(Arc::new(service)),
            running: Mutex::new(HashMap::new()),
            llm: Arc::new(FoundationModelsProvider::new(FoundationModelsConfig::new(
                "/nonexistent/fm",
                std::env::temp_dir(),
            ))),
        })
    }

    async fn pending(chat: &Chat) -> i64 {
        let service = chat.service.clone().unwrap();
        let c = service.current().await.unwrap();
        service.ask(c.id, "Qual a carência?").await.unwrap().1
    }

    fn recorder() -> (
        Arc<Mutex<Vec<AnswerEvent>>>,
        impl Fn(AnswerEvent) + Send + Sync,
    ) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        (events, move |e| sink.lock().unwrap().push(e))
    }

    #[tokio::test]
    async fn streams_tokens_then_done_and_saves_the_answer() {
        let chat = chat(FakeLlmProvider::available().answering("A carência é de 180 dias [1]."));
        let id = pending(&chat).await;
        let (events, emit) = recorder();
        run_answer(&chat, id, &emit).await.unwrap();
        {
            let events = events.lock().unwrap();
            let text: String = events
                .iter()
                .filter_map(|e| match e {
                    AnswerEvent::Token { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(text, "A carência é de 180 dias [1].");
            assert!(
                matches!(events.last(), Some(AnswerEvent::Done { status }) if status == "answered")
            );
        }
        assert!(chat.running.lock().unwrap().is_empty(), "flag released");

        // Asking for the same answer again just reports it as final.
        let (again, emit) = recorder();
        run_answer(&chat, id, &emit).await.unwrap();
        assert!(
            matches!(again.lock().unwrap().as_slice(), [AnswerEvent::Done { status }] if status == "final")
        );
    }

    #[tokio::test]
    async fn a_running_answer_is_not_generated_twice() {
        let chat = chat(FakeLlmProvider::available().answering("x"));
        let id = pending(&chat).await;
        chat.running.lock().unwrap().insert(id, Default::default());
        let err = run_answer(&chat, id, &|_| {}).await.unwrap_err();
        assert_eq!(err.code, "running");
    }

    #[tokio::test]
    async fn cancelling_keeps_the_partial_answer() {
        let chat = chat(FakeLlmProvider::available().answering("um dois três quatro cinco"));
        let id = pending(&chat).await;
        let for_cancel = chat.clone();
        let emit = move |e: AnswerEvent| {
            if matches!(e, AnswerEvent::Token { .. }) {
                let _ = run_cancel(&for_cancel, id);
            }
        };
        run_answer(&chat, id, &emit).await.unwrap();
        let m = chat.service.clone().unwrap().message(id).await.unwrap();
        assert_eq!(m.status, MessageStatus::Cancelled);
        assert_eq!(m.content, "um");
        assert_eq!(
            run_cancel(&chat, id).unwrap_err().code,
            "chat",
            "nothing running anymore"
        );
    }

    #[tokio::test]
    async fn errors_are_code_and_message() {
        let chat = chat(FakeLlmProvider::available());
        let err = run_answer(&chat, 999, &|_| {}).await.unwrap_err();
        assert_eq!(err.code, "chat");
        assert!(!err.message.is_empty());
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["code"], "chat");
    }

    #[test]
    fn commands_round_trip_through_the_tauri_mock_runtime() {
        use tauri::{
            Manager,
            test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets},
            webview::InvokeRequest,
        };
        let app = mock_builder()
            .invoke_handler(tauri::generate_handler![
                super::app_info,
                super::cancel_answer
            ])
            .build(mock_context(noop_assets()))
            .expect("mock app");
        app.manage(Arc::into_inner(chat(FakeLlmProvider::available())).unwrap());
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let invoke = |cmd: &str, body: serde_json::Value| InvokeRequest {
            cmd: cmd.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        };
        let info = get_ipc_response(&webview, invoke("app_info", serde_json::json!({})))
            .expect("app_info")
            .deserialize::<serde_json::Value>()
            .unwrap();
        assert!(info["version"].is_string());
        let err = get_ipc_response(
            &webview,
            invoke("cancel_answer", serde_json::json!({ "messageId": 7 })),
        )
        .unwrap_err();
        assert_eq!(err["code"], "chat");
    }
}
