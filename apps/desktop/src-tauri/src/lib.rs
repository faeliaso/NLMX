//! Tauri shell. Opens the main window on the `nlmx://` custom scheme, whose requests are
//! answered in-process by the `ui-web` router (no TCP server, ADR 0003).

mod commands;
mod legacy_data;
mod protocol;
pub mod self_check;
mod wiring;

use tauri::{Manager, TitleBarStyle, WebviewUrl, WebviewWindowBuilder};

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .register_asynchronous_uri_scheme_protocol(protocol::SCHEME, protocol::handle)
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::open_data_dir,
            commands::open_logs_dir,
            commands::open_download_page,
            commands::report_client_error,
            commands::import_documents,
            commands::retry_document,
            commands::retry_failed,
            commands::embed_pending_now,
            commands::reindex_all,
            commands::download_model,
            commands::cancel_download,
            commands::activate_model,
            commands::remove_model,
            commands::verify_model,
            commands::answer_message,
            commands::cancel_answer,
        ])
        .setup(|app| {
            // Composition happens here because the data directory comes from the app handle.
            let data_dir = app.path().app_data_dir()?;
            // Data left by the app under its previous name moves here before anything opens it.
            let migration = legacy_data::migrate(&data_dir);
            // Structured logs (JSON, redacted) and local metrics, before anything else logs.
            let diagnostics = nlmx_telemetry::init(nlmx_telemetry::TelemetryConfig::new(
                wiring::data_layout(&data_dir),
            ));
            migration.log();
            tauri::async_runtime::spawn(nlmx_telemetry::run_sampler(
                diagnostics.clone(),
                std::time::Duration::from_secs(60),
            ));
            let services = wiring::build(&data_dir, Some(diagnostics));
            // In the background: finish ingestions interrupted by a previous quit, then embed
            // whatever is still waiting for embeddings (if a model is configured).
            let ingestion = services.ingestion.0.clone().ok();
            let embedder = services.embedder.0.clone();
            let remover = services.remover.clone().ok();
            let running = services.indexing.activity.begin();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // Library files of removed documents whose deletion failed, before any import.
                if let Some(remover) = remover {
                    match remover.prune_library().await {
                        Ok(0) => {}
                        Ok(count) => tracing::info!(count, "orphan library files removed"),
                        Err(err) => tracing::warn!("library cleanup failed: {}", err.message),
                    }
                }
                if let Some(ingestion) = ingestion {
                    let resumed = ingestion.resume().await;
                    if !resumed.is_empty() {
                        tracing::info!(count = resumed.len(), "resumed interrupted ingestions");
                    }
                }
                if let Some(embedder) = embedder {
                    let embedded = embedder.embed_pending().await;
                    if !embedded.is_empty() {
                        tracing::info!(count = embedded.len(), "embedded pending documents");
                    }
                }
                drop(running);
                wiring::notify_indexing_changed(&handle);
            });
            // Report installed models that are corrupted or have an update (never downloads).
            let models = services.models.provider.clone();
            tauri::async_runtime::spawn(
                async move { wiring::report_models(models.as_ref()).await },
            );

            app.manage(services.ui);
            app.manage(services.ingestion);
            app.manage(services.embeddings);
            app.manage(services.embedder);
            app.manage(services.indexing);
            app.manage(services.models);
            app.manage(services.chat);
            app.manage(services.diagnostics);

            WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::CustomProtocol(protocol::start_url()),
            )
            .title("NLMX")
            .inner_size(1200.0, 800.0)
            .min_inner_size(820.0, 520.0)
            .title_bar_style(TitleBarStyle::Overlay)
            .hidden_title(true)
            .build()?;
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "main window created");
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building NLMX")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // Never leave llama-server or fm serve running after the app quits.
                if let Some(provider) = app.state::<std::sync::Arc<wiring::EmbeddingSlot>>().llama()
                {
                    tauri::async_runtime::block_on(provider.shutdown());
                }
                tauri::async_runtime::block_on(app.state::<wiring::Chat>().llm.shutdown());
            }
        });
}
