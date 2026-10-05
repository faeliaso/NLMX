//! Presentation layer. Exposes an axum [`Router`] that is driven in-process by the Tauri
//! custom URI scheme — there is no TCP server (ADR 0003). Handlers call application use
//! cases and return full pages or HTMX fragments rendered with askama.

mod assets;
mod chat;
mod diagnostics;
mod documents;
mod error;
mod formats;
#[cfg(debug_assertions)]
mod gallery;
mod indexing;
mod markdown;
mod models;
mod sections;
mod shell;
mod sources;
mod status;
mod viewer;

use std::sync::Arc;

use axum::routing::post;
use axum::{Router, http::HeaderValue, middleware, response::Response, routing::get};
use nlmx_application::{
    ports::{Diagnostics, ModelProvider, NoteSubmitter},
    use_cases::{
        ChatService, DocumentIngestion, GetSystemStatus, Indexing, RemoveDocument, ViewDocument,
    },
};

pub use error::FALLBACK_ERROR_HTML;

/// Where new versions are published (set at build time; no automatic update check).
pub const DOWNLOAD_URL: Option<&str> = option_env!("NLMX_DOWNLOAD_URL");

const CONTENT_SECURITY_POLICY: &str =
    "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:";

/// Use cases the UI depends on, injected by the composition root.
#[derive(Clone)]
pub struct AppState {
    pub system_status: Arc<GetSystemStatus>,
    /// Unavailable (with the reason) when the database or the PDF engine failed to load.
    pub ingestion: Result<Arc<DocumentIngestion>, String>,
    /// Conversations answered from the documents (unavailable without the database).
    pub chat: Result<Arc<ChatService>, String>,
    /// The PDF viewer (unavailable without the database or the PDF engine).
    pub viewer: Result<Arc<ViewDocument>, String>,
    /// Removes documents with everything derived from them (unavailable without the database).
    pub remover: Result<Arc<RemoveDocument>, String>,
    /// Local session measurements (`None` hides the diagnostics section).
    pub diagnostics: Option<Arc<dyn Diagnostics>>,
    /// Embedding model files (catalog, downloads); `None` hides the model list.
    pub models: Option<Arc<dyn ModelProvider>>,
    /// The state of the index and the background work (unavailable without the database).
    pub indexing: Result<Arc<Indexing>, String>,
    /// Adds pasted notes to the import queue (`None` hides "Adicionar nota").
    pub notes: Option<Arc<dyn NoteSubmitter>>,
}

/// Builds the UI router served under the app's custom scheme.
pub fn router(state: AppState) -> Router {
    let router = Router::new()
        // The app starts directly in Chat.
        .route("/", get(chat::current))
        .route("/chat", get(chat::current))
        .route("/chat/new", post(chat::new))
        .route("/chat/{id}", get(chat::conversation))
        .route("/chat/{id}/scope", post(chat::scope))
        .route("/chat/{id}/delete", post(chat::delete))
        .route("/chat/{id}/messages", post(chat::ask))
        .route("/chat/messages/{id}", get(chat::answer))
        .route("/chat/messages/{id}/regenerate", post(chat::regenerate))
        .route("/chat/messages/{id}/free", post(chat::answer_freely))
        .route("/viewer/{doc}", get(viewer::open))
        .route("/sources/{id}", get(sources::open))
        .route("/viewer/{doc}/pages/{page}/text", get(viewer::text))
        .route("/viewer/{doc}/search", get(viewer::search))
        .route("/documents/{id}/pages/{file}", get(viewer::page_image))
        .route("/documents", get(sections::documents))
        .route("/documents/notes", post(sections::add_note))
        .route("/documents/{id}/delete", post(sections::remove_document))
        .route("/indexing", get(indexing::page))
        .route("/models", get(models::page))
        .route("/settings", get(sections::settings))
        .route("/fragments/status", get(status::fragment))
        .route("/fragments/documents", get(sections::documents_fragment))
        .route("/fragments/models", get(models::fragment))
        .route("/fragments/indexing", get(indexing::fragment))
        .route("/assets/app.css", get(assets::app_css))
        .route("/assets/htmx.min.js", get(assets::htmx_js))
        .route("/assets/ds.js", get(assets::ds_js))
        .route("/assets/app.js", get(assets::app_js))
        .route("/assets/viewer.js", get(assets::viewer_js));

    #[cfg(debug_assertions)]
    let router = router.route("/design-system", get(gallery::page));

    router
        .fallback(sections::not_found)
        .layer(middleware::map_response(add_security_headers))
        .with_state(state)
}

async fn add_security_headers(mut response: Response) -> Response {
    response.headers_mut().insert(
        http::header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use nlmx_application::use_cases::DocumentIngestion;
    use nlmx_domain::chat::ConversationScope;
    use nlmx_domain::document::{BoundingBox, DocumentMetadata, TextSpan};
    use nlmx_domain::generation::LanguageModelStatus;
    use nlmx_testing::{
        FakeChunker, FakeDocumentEngine, FakeDocumentRepository, FakeFileStore, FakeLlmProvider,
        FakePage, FakeStorage, FakeStructureAnalyzer, WordTokenCounter,
    };
    use tower::ServiceExt;

    fn status(lm: LanguageModelStatus, storage: FakeStorage) -> Arc<GetSystemStatus> {
        Arc::new(GetSystemStatus::new(
            Arc::new(FakeLlmProvider::new(lm)),
            Arc::new(storage),
            Ok("chromium/7881".into()),
            Arc::new(nlmx_testing::FakeRuntime::llama()),
        ))
    }

    fn app_with(lm: LanguageModelStatus, storage: FakeStorage) -> Router {
        router(AppState {
            system_status: status(lm, storage),
            ingestion: Ok(Arc::new(ingestion(
                FakeFileStore::default(),
                FakeDocumentEngine::default(),
            ))),
            chat: Ok(chat_service(FakeLlmProvider::available())),
            viewer: Err("visualizador indisponível nos testes".into()),
            remover: Err("remoção indisponível neste teste".into()),
            diagnostics: None,
            models: None,
            indexing: Err("indexação indisponível neste teste".into()),
            notes: None,
        })
    }

    fn ingestion(files: FakeFileStore, engine: FakeDocumentEngine) -> DocumentIngestion {
        DocumentIngestion {
            pipeline: None,
            progress: None,
            viewer: None,
            engine: Arc::new(engine),
            files: Arc::new(files),
            documents: Arc::new(FakeDocumentRepository::default()),
            analyzer: Arc::new(FakeStructureAnalyzer),
            chunker: Arc::new(FakeChunker),
            tokens: Arc::new(WordTokenCounter),
            policy: Default::default(),
            embedder: None,
        }
    }

    fn app(status: LanguageModelStatus) -> Router {
        app_with(status, FakeStorage::healthy())
    }

    async fn send(router: Router, path: &str, htmx: bool) -> (StatusCode, http::HeaderMap, String) {
        let mut request = Request::get(path);
        if htmx {
            request = request.header("HX-Request", "true");
        }
        let response = router
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, headers, String::from_utf8_lossy(&body).into_owned())
    }

    async fn get(path: &str) -> (StatusCode, http::HeaderMap, String) {
        send(app(LanguageModelStatus::Available), path, false).await
    }

    #[tokio::test]
    async fn root_opens_the_shell_on_chat() {
        let (status, headers, body) = get("/").await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            headers[http::header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("default-src 'self'")
        );
        assert!(body.contains(r#"<script src="/assets/htmx.min.js""#));
        assert!(body.contains(r#"href="/assets/app.css""#));
        assert!(body.contains(r#"id="sidebar-nav""#));
        assert!(body.contains(r#"hx-get="/fragments/status""#));
        let chat_link = body.find(r#"href="/chat""#).unwrap();
        let current = body.find(r#"aria-current="page""#).unwrap();
        assert!(
            current > chat_link && current - chat_link < 300,
            "Chat should be the current section"
        );
    }

    #[tokio::test]
    async fn every_section_renders_as_a_full_page() {
        for path in ["/chat", "/documents", "/indexing", "/models", "/settings"] {
            let (status, _, body) = get(path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(body.contains("<html"), "{path} should be a full page");
            assert!(
                body.contains("data-page-title"),
                "{path} should have a contextual header"
            );
        }
    }

    #[tokio::test]
    async fn htmx_navigation_returns_content_and_out_of_band_nav() {
        let (status, _, body) = send(app(LanguageModelStatus::Available), "/documents", true).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!body.contains("<html"));
        assert!(body.contains("Documentos"));
        assert!(body.contains(r#"hx-swap-oob="true""#));
    }

    #[tokio::test]
    async fn unknown_route_renders_an_error_state_with_404() {
        let (status, _, body) = get("/nope").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains(r#"role="alert""#));
        assert!(body.contains("<html"));

        let (status, _, body) = send(app(LanguageModelStatus::Available), "/nope", true).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(!body.contains("<html"));
    }

    #[tokio::test]
    async fn status_fragment_reflects_the_language_model() {
        let (_, _, body) = send(
            app(LanguageModelStatus::Available),
            "/fragments/status",
            true,
        )
        .await;
        assert!(body.contains("Disponível"));

        let (_, _, body) = send(
            app(LanguageModelStatus::LicenseRequired),
            "/fragments/status",
            true,
        )
        .await;
        assert!(body.contains("Licença pendente"));

        let unavailable = LanguageModelStatus::Unavailable {
            kind: nlmx_domain::generation::UnavailableKind::Other,
            reason: "Falha ao carregar o modelo".into(),
        };
        let (_, _, body) = send(app(unavailable), "/models", true).await;
        assert!(body.contains("Falha ao carregar o modelo"));

        let disabled = LanguageModelStatus::Unavailable {
            kind: nlmx_domain::generation::UnavailableKind::AppleIntelligenceDisabled,
            reason: "Apple Intelligence is not enabled".into(),
        };
        let (_, _, body) = send(app(disabled), "/models", true).await;
        assert!(body.contains("Ative o Apple Intelligence em Ajustes do Sistema"));

        let incompatible = LanguageModelStatus::Incompatible {
            reason: "Requer macOS 27 ou superior.".into(),
        };
        let (_, _, body) = send(app(incompatible), "/models", true).await;
        assert!(body.contains("Incompatível") && body.contains("Requer macOS 27"));
    }

    struct FixedDiagnostics(nlmx_application::ports::DiagnosticsSnapshot);

    impl Diagnostics for FixedDiagnostics {
        fn snapshot(&self) -> nlmx_application::ports::DiagnosticsSnapshot {
            self.0.clone()
        }
        fn sample(&self) -> nlmx_application::ports::BoxFuture<'_, ()> {
            Box::pin(async {})
        }
    }

    #[tokio::test]
    async fn settings_show_session_diagnostics_without_content() {
        use nlmx_application::ports::{
            DiagnosticsSnapshot, OperationStats, ResourceUsage, StorageUsage,
        };
        use nlmx_domain::telemetry::Operation;
        let snapshot = DiagnosticsSnapshot {
            uptime_secs: 600,
            operations: vec![OperationStats {
                operation: Operation::Ingest,
                total: 4,
                failed: 1,
                p50_ms: Some(820),
                p95_ms: Some(2400),
                max_ms: Some(2400),
            }],
            documents_imported: 3,
            pages: 120,
            chunks: 340,
            pages_per_second: Some(48.5),
            embeddings: 340,
            embeddings_per_second: Some(85.0),
            first_token_p50_ms: Some(640),
            resources: Some(ResourceUsage {
                rss_bytes: 200 * 1024 * 1024,
                llama_rss_bytes: Some(700 * 1024 * 1024),
                fm_rss_bytes: None,
            }),
            storage: Some(StorageUsage {
                database_bytes: 3 * 1024 * 1024,
                library_bytes: 10 * 1024 * 1024,
                models_bytes: 0,
                logs_bytes: 1024,
            }),
        };
        let app = router(AppState {
            system_status: status(LanguageModelStatus::Available, FakeStorage::healthy()),
            ingestion: Err("x".into()),
            chat: Err("x".into()),
            viewer: Err("x".into()),
            remover: Err("remoção indisponível neste teste".into()),
            diagnostics: Some(Arc::new(FixedDiagnostics(snapshot))),
            models: None,
            indexing: Err("indexação indisponível neste teste".into()),
            notes: None,
        });
        let (_, _, body) = send(app, "/settings", true).await;
        assert!(body.contains("Diagnóstico") && body.contains("10 min"));
        assert!(body.contains("3 · 120 páginas · 340 trechos"));
        assert!(body.contains("48,5/s") && body.contains("85,0/s"));
        assert!(
            body.contains("Importação") && body.contains("25,0%"),
            "error rate: {body}"
        );
        assert!(body.contains("820 ms") && body.contains("2,4 s"));
        assert!(body.contains("700,0 MB") && body.contains("13,0 MB"));
        assert!(body.contains(r#"data-copy-from="diagnostics-json""#));
        assert!(
            body.contains("pages_per_second") && body.contains(": 48.5"),
            "JSON for copying"
        );
    }

    #[tokio::test]
    async fn settings_show_database_diagnostics_and_status_bar_flags_failures() {
        let (_, _, body) = get("/settings").await;
        assert!(body.contains("/tmp/nlmx-test.sqlite3"));
        assert!(body.contains("5 de 5"));
        assert!(body.contains("PDFium chromium/7881"));
        assert!(body.contains("llama.cpp b11349") && body.contains("Incluído no app"));

        let broken = || {
            app_with(
                LanguageModelStatus::Available,
                FakeStorage::failing("disco cheio"),
            )
        };
        let (_, _, body) = send(broken(), "/settings", true).await;
        assert!(body.contains("disco cheio"));
        let (_, _, body) = send(broken(), "/fragments/status", true).await;
        assert!(body.contains("Banco de dados"));
        assert!(body.contains("disco cheio"));
    }

    #[tokio::test]
    async fn documents_page_lists_imported_documents_or_explains_why_it_cannot() {
        // Empty library: empty state with the import action.
        let (_, _, body) = get("/documents").await;
        assert!(body.contains("Nenhum documento"));
        // The import action is the one in the page header, not another in the empty state.
        assert_eq!(
            body.matches(r#"data-command="import_documents""#).count(),
            1,
            "{body}"
        );

        // One imported document.
        let files = FakeFileStore::default();
        let sha = files.add("/in/contrato.pdf", b"pdf");
        let span = TextSpan {
            text: "Carência.".into(),
            bbox: BoundingBox {
                left: 0.0,
                top: 0.0,
                right: 1.0,
                bottom: 1.0,
            },
            font_name: "Helvetica".into(),
            font_size: 11.0,
            bold: false,
            italic: false,
        };
        let engine = FakeDocumentEngine::default().with_document(
            FakeFileStore::library_path(&sha),
            DocumentMetadata {
                title: Some("Contrato".into()),
                ..Default::default()
            },
            vec![FakePage {
                spans: vec![span],
                images: vec![],
            }],
        );
        let ingestion = Arc::new(ingestion(files, engine));
        ingestion
            .import(std::path::Path::new("/in/contrato.pdf"))
            .await;
        let app = router(AppState {
            system_status: status(LanguageModelStatus::Available, FakeStorage::healthy()),
            ingestion: Ok(ingestion),
            chat: Ok(chat_service(FakeLlmProvider::available())),
            viewer: Err("visualizador indisponível nos testes".into()),
            remover: Err("remoção indisponível neste teste".into()),
            diagnostics: None,
            models: None,
            indexing: Err("indexação indisponível neste teste".into()),
            notes: None,
        });
        let (_, _, body) = send(app.clone(), "/documents", true).await;
        assert!(body.contains("Contrato") && body.contains("contrato.pdf"));
        assert!(body.contains("1 página") && body.contains("1 trecho ·"));
        assert!(body.contains("Aguardando embeddings"));
        let (status_code, _, fragment) = send(app, "/fragments/documents", true).await;
        assert_eq!(status_code, StatusCode::OK);
        assert!(fragment.contains("Contrato") && !fragment.contains("page-header"));

        // No database / PDF engine.
        let unavailable = router(AppState {
            system_status: status(LanguageModelStatus::Available, FakeStorage::healthy()),
            ingestion: Err("PDFium não encontrado".into()),
            chat: Ok(chat_service(FakeLlmProvider::available())),
            viewer: Err("visualizador indisponível nos testes".into()),
            remover: Err("remoção indisponível neste teste".into()),
            diagnostics: None,
            models: None,
            indexing: Err("indexação indisponível neste teste".into()),
            notes: None,
        });
        let (_, _, body) = send(unavailable, "/documents", true).await;
        assert!(body.contains("Biblioteca indisponível") && body.contains("PDFium não encontrado"));
        assert!(!body.contains(r#"data-command="import_documents""#));
    }

    #[tokio::test]
    async fn indexing_shows_the_index_and_what_needs_attention() {
        use nlmx_application::{
            ports::{DocumentRepository, NewDocument},
            use_cases::{EmbedDocuments, IndexingActivity},
        };
        use nlmx_domain::ingestion::DocumentStatus;

        let documents = Arc::new(FakeDocumentRepository::default());
        for sha in ['a', 'b'] {
            documents
                .insert(NewDocument {
                    sha256: sha.to_string().repeat(64),
                    original_filename: format!("{sha}.pdf"),
                    original_path: String::new(),
                    library_path: String::new(),
                    file_size: 1,
                    document_type: nlmx_domain::document_type::DocumentType::Pdf,
                    note_text: None,
                })
                .await
                .unwrap();
        }
        documents
            .set_status(
                1,
                DocumentStatus::Failed,
                Some("PDF protegido por senha".into()),
            )
            .await
            .unwrap();
        let indexing = |documents: Arc<FakeDocumentRepository>| {
            let source = nlmx_testing::FixedEmbeddingSource::none();
            Arc::new(Indexing {
                reader: documents.clone(),
                documents: documents.clone(),
                embeddings: source.clone(),
                embedder: Arc::new(EmbedDocuments {
                    progress: None,
                    embeddings: source,
                    vectors: Arc::new(nlmx_testing::FakeVectorStore::default()),
                    chunks: Arc::new(nlmx_testing::FakeCorpus::default()),
                    documents,
                    batch_size: 8,
                }),
                ingestion: Ok(Arc::new(ingestion(
                    FakeFileStore::default(),
                    FakeDocumentEngine::default(),
                ))),
                activity: Arc::new(IndexingActivity::default()),
            })
        };
        let app = |indexing: Result<Arc<Indexing>, String>| {
            router(AppState {
                system_status: status(LanguageModelStatus::Available, FakeStorage::healthy()),
                ingestion: Err("não usado neste teste".into()),
                chat: Err("não usado neste teste".into()),
                viewer: Err("não usado neste teste".into()),
                remover: Err("não usado neste teste".into()),
                diagnostics: None,
                models: None,
                indexing,
                notes: None,
            })
        };

        // `b` is still being read: the page refreshes itself.
        let (_, _, body) = send(app(Ok(indexing(documents.clone()))), "/indexing", true).await;
        assert!(body.contains("Busca só por palavras-chave"));
        assert!(!body.contains("Em andamento") && !body.contains("Processando agora"));
        assert!(
            !body.contains("progressbar"),
            "no progress bars on this page"
        );
        assert!(body.contains("Precisa de atenção") && body.contains("PDF protegido por senha"));
        assert!(body.contains(r#"data-command="retry_document" data-command-args='{"id":1}'"#));
        assert!(body.contains(r#"data-command="retry_failed""#));
        assert!(
            !body.contains("reindex_all"),
            "no model, nothing to reindex"
        );
        assert!(body.contains("every 2s,"));

        // Once nothing is running, it stops polling; the fragment has no page header.
        documents
            .set_status(2, DocumentStatus::NeedsOcr, None)
            .await
            .unwrap();
        let (status_code, _, fragment) =
            send(app(Ok(indexing(documents))), "/fragments/indexing", true).await;
        assert_eq!(status_code, StatusCode::OK);
        assert!(!fragment.contains("every 2s") && !fragment.contains("page-header"));
        assert!(fragment.contains("Sem texto (OCR)"));

        let (_, _, body) = send(
            app(Err("Banco de dados indisponível".into())),
            "/indexing",
            true,
        )
        .await;
        assert!(
            body.contains("Indexação indisponível") && body.contains("Banco de dados indisponível")
        );
    }

    #[tokio::test]
    async fn documents_can_be_removed_after_confirming() {
        let files = Arc::new(FakeFileStore::default());
        let sha = files.add("/in/contrato.pdf", b"pdf");
        let span = TextSpan {
            text: "Carência de 180 dias.".into(),
            bbox: BoundingBox {
                left: 0.0,
                top: 0.0,
                right: 1.0,
                bottom: 1.0,
            },
            font_name: "Helvetica".into(),
            font_size: 11.0,
            bold: false,
            italic: false,
        };
        let engine = FakeDocumentEngine::default().with_document(
            FakeFileStore::library_path(&sha),
            DocumentMetadata {
                title: Some("Contrato".into()),
                ..Default::default()
            },
            vec![FakePage {
                spans: vec![span],
                images: vec![],
            }],
        );
        let documents = Arc::new(FakeDocumentRepository::default());
        let ingestion = Arc::new(DocumentIngestion {
            pipeline: None,
            progress: None,
            viewer: None,
            engine: Arc::new(engine),
            files: files.clone(),
            documents: documents.clone(),
            analyzer: Arc::new(FakeStructureAnalyzer),
            chunker: Arc::new(FakeChunker),
            tokens: Arc::new(WordTokenCounter),
            policy: Default::default(),
            embedder: None,
        });
        ingestion
            .import(std::path::Path::new("/in/contrato.pdf"))
            .await;
        let app = router(AppState {
            system_status: status(LanguageModelStatus::Available, FakeStorage::healthy()),
            ingestion: Ok(ingestion),
            chat: Ok(chat_service(FakeLlmProvider::available())),
            viewer: Err("visualizador indisponível nos testes".into()),
            remover: Ok(Arc::new(RemoveDocument {
                documents: documents.clone(),
                files: files.clone(),
                viewer: None,
            })),
            diagnostics: None,
            models: None,
            indexing: Err("indexação indisponível neste teste".into()),
            notes: None,
        });

        let (_, _, body) = send(app.clone(), "/documents", true).await;
        assert!(body.contains(r#"data-dialog-open="remove-doc-1""#));
        assert!(body.contains("seu único trecho e os índices de busca serão apagados"));
        assert!(body.contains("O arquivo original não é afetado."));
        assert!(body.contains(r#"hx-post="/documents/1/delete""#));

        // While it is being read, removal is not offered.
        documents.force_status(1, nlmx_domain::ingestion::DocumentStatus::Extracting);
        let (_, _, body) = send(app.clone(), "/documents", true).await;
        assert!(!body.contains(r#"data-dialog-open="remove-doc-1""#));
        assert!(body.contains("Disponível quando a leitura do documento terminar"));
        let (status_code, body) = post_form(app.clone(), "/documents/1/delete", "").await;
        assert_eq!(status_code, StatusCode::OK);
        assert!(body.contains("ainda está sendo processado"), "{body}");
        assert!(body.contains(r#"data-toast-on-load="danger""#), "{body}");
        assert!(
            !body.contains("alert-danger"),
            "an error is a toast, not a banner"
        );
        assert!(files.removed().is_empty());

        documents.force_status(1, nlmx_domain::ingestion::DocumentStatus::Embedding);
        let (status_code, body) = post_form(app.clone(), "/documents/1/delete", "").await;
        assert_eq!(status_code, StatusCode::OK);
        assert!(body.contains(r#"data-toast-on-load="success""#), "{body}");
        assert!(body.contains("Documento removido."), "{body}");
        assert!(
            !body.contains("alert-success"),
            "the outcome is a toast, not a banner"
        );
        assert!(body.contains("Nenhum documento"), "the list is empty again");
        assert_eq!(files.removed(), [sha]);

        let (_, body) = post_form(app, "/documents/1/delete", "").await;
        assert!(body.contains("O documento não existe mais."));
        assert!(body.contains(r#"data-toast-on-load="danger""#));
    }

    fn descriptor(id: &str) -> nlmx_domain::models::ModelDescriptor {
        nlmx_domain::models::ModelDescriptor {
            id: id.into(),
            display_name: format!("Modelo {id}"),
            description: "Multilíngue".into(),
            version: "v1".into(),
            url: "https://example.com/m.gguf".into(),
            file_name: "m.gguf".into(),
            size: 639_150_592,
            sha256: "0".repeat(64),
            license: nlmx_domain::models::License {
                id: "Apache-2.0".into(),
                url: "https://example.com/l".into(),
            },
            languages: vec!["pt".into()],
            dimensions: 1024,
            pooling: "last".into(),
            query_prefix: String::new(),
            passage_prefix: String::new(),
            context_size: 8192,
            recommended: true,
        }
    }

    async fn post_form(router: Router, path: &str, form: &str) -> (StatusCode, String) {
        let request = Request::post(path)
            .header("HX-Request", "true")
            .header(
                http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(Body::from(form.to_string()))
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    fn chat_on(
        conversations: Arc<nlmx_testing::FakeConversations>,
        llm: FakeLlmProvider,
    ) -> Arc<ChatService> {
        use nlmx_application::services::{
            free_chat::FreeChat,
            rag::{RagEngine, RagOptions},
            retrieval::HybridRetriever,
            retriever::{Retriever, RetrieverOptions},
        };
        use nlmx_testing::{FakeCorpus, FakeVectorStore, FixedEmbeddingSource};
        let corpus = Arc::new(
            FakeCorpus::default()
                .chunk(1, 1, 2, "A carência é de 180 dias.")
                .in_section("3. Prazos")
                .chunk(2, 1, 3, "Cobertura de exames.")
                .in_section("4. Coberturas"),
        );
        let hybrid = HybridRetriever::new(
            FixedEmbeddingSource::none(),
            Arc::new(FakeVectorStore::default()),
            corpus.clone(),
            corpus.clone(),
        );
        let llm = Arc::new(llm);
        Arc::new(ChatService {
            conversations,
            rag: Arc::new(RagEngine::new(
                Arc::new(Retriever::new(Arc::new(hybrid))),
                corpus,
                llm.clone(),
            )),
            free: Arc::new(FreeChat::new(llm)),
            options: RagOptions {
                retriever: RetrieverOptions {
                    min_score: 0.0,
                    ..Default::default()
                },
                min_relevance: 0.1,
                ..Default::default()
            },
        })
    }

    fn chat_service(llm: FakeLlmProvider) -> Arc<ChatService> {
        chat_on(Arc::default(), llm)
    }

    /// An app whose library has one imported document (so the composer is enabled).
    async fn chat_app(chat: Arc<ChatService>, viewer: Result<Arc<ViewDocument>, String>) -> Router {
        chat_app_with(LanguageModelStatus::Available, chat, viewer).await
    }

    async fn chat_app_with(
        lm: LanguageModelStatus,
        chat: Arc<ChatService>,
        viewer: Result<Arc<ViewDocument>, String>,
    ) -> Router {
        let files = FakeFileStore::default();
        let sha = files.add("/in/a.pdf", b"pdf");
        let span = TextSpan {
            text: "Texto.".into(),
            bbox: BoundingBox {
                left: 0.0,
                top: 0.0,
                right: 1.0,
                bottom: 1.0,
            },
            font_name: "Helvetica".into(),
            font_size: 11.0,
            bold: false,
            italic: false,
        };
        let engine = FakeDocumentEngine::default().with_document(
            FakeFileStore::library_path(&sha),
            DocumentMetadata::default(),
            vec![FakePage {
                spans: vec![span],
                images: vec![],
            }],
        );
        let ingestion = Arc::new(ingestion(files, engine));
        ingestion.import(std::path::Path::new("/in/a.pdf")).await;
        router(AppState {
            system_status: status(lm, FakeStorage::healthy()),
            ingestion: Ok(ingestion),
            chat: Ok(chat),
            viewer,
            remover: Err("remoção indisponível neste teste".into()),
            diagnostics: None,
            models: None,
            indexing: Err("indexação indisponível neste teste".into()),
            notes: None,
        })
    }

    #[tokio::test]
    async fn a_new_chat_is_a_free_conversation_even_without_documents() {
        let chat = chat_service(FakeLlmProvider::available());
        let app = router(AppState {
            system_status: status(LanguageModelStatus::Available, FakeStorage::healthy()),
            ingestion: Err("sem documentos".into()),
            chat: Ok(chat),
            viewer: Err("x".into()),
            remover: Err("x".into()),
            diagnostics: None,
            models: None,
            indexing: Err("indexação indisponível neste teste".into()),
            notes: None,
        });
        let (_, _, body) = send(app, "/chat", true).await;
        assert!(body.contains(r#"data-scope="free""#), "{body}");
        assert!(body.contains(r#"<option value="free" selected>Conversa livre</option>"#));
        assert!(
            !body.contains("Todos os documentos"),
            "no documents to choose"
        );
        assert!(body.contains(r#"hx-post="/chat/1/messages""#));
        assert!(body.contains(r#"placeholder="Pergunte qualquer coisa…""#));
        assert!(!body.contains("disabled placeholder"));
        assert!(body.contains("Converse livremente") && body.contains("Ir para Documentos"));
        assert!(!body.contains("data-fill-question"), "document suggestions");
    }

    #[tokio::test]
    async fn chat_scope_selector_offers_free_and_the_documents() {
        let app = chat_app(chat_service(FakeLlmProvider::available()), Err("x".into())).await;
        let (_, _, body) = send(app.clone(), "/chat", true).await;
        assert!(body.contains(r#"hx-post="/chat/1/messages""#), "{body}");
        assert!(
            body.contains(r#"hx-swap="beforeend""#),
            "turns are appended to the log"
        );
        // Scope selector, new conversation, recent conversations.
        assert!(body.contains(r#"name="scope""#) && body.contains("Conversa livre"));
        assert!(body.contains(r#"<optgroup label="Documentos">"#));
        assert!(body.contains(r#"<option value="all">Todos os documentos</option>"#));
        assert!(!body.contains("Ir para Documentos"));

        let (_, body) = post_form(app, "/chat/1/scope", "scope=all").await;
        assert!(body.contains(r#"data-scope="documents""#));
        assert!(body.contains(r#"<option value="all" selected>Todos os documentos</option>"#));
        assert!(body.contains(r#"placeholder="Pergunte algo sobre seus documentos…""#));
        assert!(body.contains(r#"data-fill-question="Explique este documento.""#));
        assert!(!body.contains("principais pontos da seção 3"));
        assert!(body.contains(r#"hx-post="/chat/new""#));
        assert!(body.contains(r#"popovertarget="chat-recent""#));
        assert!(body.contains(r#"class="btn btn-primary btn-icon composer-action""#));
    }

    #[tokio::test]
    async fn asking_returns_a_streaming_turn_and_the_final_answer_has_clickable_citations() {
        let chat = chat_service(
            FakeLlmProvider::available()
                .answering("A carência é de **180 dias** [1]. <script>x</script>"),
        );
        let app = chat_app(chat.clone(), Err("x".into())).await;
        send(app.clone(), "/chat", true).await; // opens (creates) conversation 1
        post_form(app.clone(), "/chat/1/scope", "scope=all").await;
        let (status, body) =
            post_form(app.clone(), "/chat/1/messages", "q=Qual+a+car%C3%AAncia%3F").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("data-chat-turn") && body.contains("Qual a carência?"));
        assert!(body.contains(r#"data-status="streaming""#), "{body}");
        assert!(
            !body.contains("cancel_answer"),
            "the composer button stops the answer, not the turn"
        );
        let id: i64 = body
            .split(r#"data-answer=""#)
            .nth(1)
            .and_then(|r| r.split('"').next())
            .and_then(|n| n.parse().ok())
            .expect("answer id");

        // The Tauri command generates the answer; then the UI fetches the final fragment.
        chat.answer(id, &|_| {}, nlmx_application::ports::CancelFlag::default())
            .await
            .unwrap();
        let (status, _, body) = send(app.clone(), &format!("/chat/messages/{id}"), true).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains(r#"data-status="answered""#));
        assert!(body.contains("<strong>180 dias</strong>"));
        assert!(
            body.contains(&format!(
                r##"class="citation" hx-get="/viewer/1?page=2&amp;cite={id}-1" hx-target="#viewer""##
            )),
            "{body}"
        );
        assert!(!body.contains("<script>x"), "model HTML is escaped");
        assert!(
            body.contains(">Fontes<") && body.contains("documento-1.pdf") && body.contains("p. 2")
        );
        assert!(body.contains("data-copy-answer") && body.contains("data-copy-text"));
        assert!(body.contains(&format!(r#"hx-post="/chat/messages/{id}/regenerate""#)));

        // Regenerating returns the placeholder again.
        let (_, body) = post_form(app, &format!("/chat/messages/{id}/regenerate"), "").await;
        assert!(body.contains(r#"data-status="streaming""#));
    }

    fn answer_id(body: &str) -> i64 {
        body.split(r#"data-answer=""#)
            .nth(1)
            .and_then(|r| r.split('"').next())
            .and_then(|n| n.parse().ok())
            .expect("answer id")
    }

    #[tokio::test]
    async fn an_answer_not_in_the_documents_offers_to_answer_without_them() {
        let chat = chat_service(FakeLlmProvider::available().answering("Paris."));
        let app = chat_app(chat.clone(), Err("x".into())).await;
        send(app.clone(), "/chat", true).await;
        post_form(app.clone(), "/chat/1/scope", "scope=all").await;
        let (_, body) = post_form(
            app.clone(),
            "/chat/1/messages",
            "q=Qual+a+capital+da+Fran%C3%A7a%3F",
        )
        .await;
        let id = answer_id(&body);
        assert!(body.contains("Buscando nos documentos"));
        chat.answer(id, &|_| {}, Default::default()).await.unwrap();
        let (_, _, body) = send(app.clone(), &format!("/chat/messages/{id}"), true).await;
        assert!(body.contains(r#"data-status="not_found""#), "{body}");
        assert!(body.contains("Não encontrei essa informação nos documentos."));
        assert!(body.contains(&format!(r#"hx-post="/chat/messages/{id}/free""#)));
        assert!(body.contains("Responder sem os documentos"));

        let (status, body) = post_form(app.clone(), &format!("/chat/messages/{id}/free"), "").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains(r#"data-status="streaming""#));
        assert!(body.contains("Gerando resposta…"));
        chat.answer(id, &|_| {}, Default::default()).await.unwrap();
        let (_, _, body) = send(app.clone(), &format!("/chat/messages/{id}"), true).await;
        assert!(body.contains(r#"data-status="answered""#) && body.contains("Paris."));
        assert!(body.contains("Sem documentos") && !body.contains(">Fontes<"));
        assert!(!body.contains("Responder sem os documentos"));

        // Only once.
        let (status, _) = post_form(app, &format!("/chat/messages/{id}/free"), "").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn failed_answers_show_the_reason_and_retry() {
        let chat = chat_service(FakeLlmProvider::new(LanguageModelStatus::LicenseRequired));
        let app = chat_app_with(
            LanguageModelStatus::LicenseRequired,
            chat.clone(),
            Err("x".into()),
        )
        .await;
        let (_, _, page) = send(app.clone(), "/chat", true).await;
        assert!(page.contains("sudo fm license"), "model notice on the page");
        post_form(app.clone(), "/chat/1/scope", "scope=all").await;
        let (_, body) = post_form(app.clone(), "/chat/1/messages", "q=car%C3%AAncia").await;
        let id: i64 = body
            .split(r#"data-answer=""#)
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        chat.answer(id, &|_| {}, Default::default()).await.unwrap();
        let (_, _, body) = send(app, &format!("/chat/messages/{id}"), true).await;
        assert!(body.contains(r#"data-status="failed""#));
        assert!(body.contains("Não foi possível responder") && body.contains("sudo fm license"));
        assert!(body.contains("Tentar de novo"));
        assert!(
            body.contains("Trechos encontrados"),
            "the passages found are still listed"
        );
    }

    #[tokio::test]
    async fn empty_questions_are_rejected() {
        let app = chat_app(chat_service(FakeLlmProvider::available()), Err("x".into())).await;
        send(app.clone(), "/chat", true).await;
        let (status, body) = post_form(app, "/chat/1/messages", "q=++").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("role=\"alert\""));
    }

    /// A chat with one saved answer citing pages 1–2 of a 2-page document, and the viewer.
    async fn viewer_app() -> (Router, i64, i64) {
        use nlmx_application::ports::{ConversationRepository, FinishedAnswer};
        use nlmx_domain::{
            chat::{
                AnswerGrounding, ConversationScope, MessagePageRef, MessageSource, MessageStatus,
                Role,
            },
            ingestion::PageBox,
        };
        let files = FakeFileStore::default();
        let sha = files.add("/in/r.pdf", b"pdf");
        let line = |text: &str, top: f32| TextSpan {
            text: text.into(),
            bbox: BoundingBox {
                left: 61.2,
                top,
                right: 61.2 + 6.0 * text.chars().count() as f32,
                bottom: top + 12.0,
            },
            font_name: "Helvetica".into(),
            font_size: 11.0,
            bold: false,
            italic: false,
        };
        let engine = Arc::new(FakeDocumentEngine::default().with_document(
            FakeFileStore::library_path(&sha),
            DocumentMetadata::default(),
            vec![
                FakePage {
                    spans: vec![line("Introdução <b>sobre</b> a carência", 79.2)],
                    images: vec![],
                },
                FakePage {
                    spans: vec![line("3. Prazos", 79.2), line("A carência termina", 100.0)],
                    images: vec![],
                },
            ],
        ));
        let documents = Arc::new(FakeDocumentRepository::default());
        let ingestion = Arc::new(DocumentIngestion {
            pipeline: None,
            progress: None,
            viewer: None,
            engine: engine.clone(),
            files: Arc::new(files),
            documents: documents.clone(),
            analyzer: Arc::new(FakeStructureAnalyzer),
            chunker: Arc::new(FakeChunker),
            tokens: Arc::new(WordTokenCounter),
            policy: Default::default(),
            embedder: None,
        });
        ingestion.import(std::path::Path::new("/in/r.pdf")).await;
        let doc = 1;

        let conversations = Arc::new(nlmx_testing::FakeConversations::default());
        let c = conversations
            .create(ConversationScope::Library)
            .await
            .unwrap();
        conversations
            .add_message(c.id, Role::User, "Prazo?", MessageStatus::Answered, None)
            .await
            .unwrap();
        let a = conversations
            .add_message(
                c.id,
                Role::Assistant,
                "",
                MessageStatus::Streaming,
                Some(AnswerGrounding::Documents),
            )
            .await
            .unwrap();
        let boxes = vec![
            PageBox {
                page: 1,
                bbox: BoundingBox {
                    left: 61.2,
                    top: 79.2,
                    right: 306.0,
                    bottom: 158.4,
                },
            },
            PageBox {
                page: 2,
                bbox: BoundingBox {
                    left: 61.2,
                    top: 396.0,
                    right: 306.0,
                    bottom: 475.2,
                },
            },
        ];
        let source = MessageSource {
            n: 1,
            cited: true,
            document_id: doc,
            chunk_id: Some(1),
            document_title: "Relatório".into(),
            page_start: 1,
            page_end: 2,
            section: Some("3. Prazos".into()),
            label: "relatorio.pdf · pp. 1–2".into(),
            quote: "A carência termina após 180 dias.".into(),
            bboxes: boxes.clone(),
            reference: nlmx_domain::source::SourceReference::pdf(
                doc,
                "Relatório",
                Some(1),
                1,
                2,
                boxes,
            ),
            document_name: "relatorio.pdf".into(),
        };
        conversations
            .finish_message(
                a,
                FinishedAnswer {
                    content: "Termina após 180 dias [1], ver [página 2].",
                    status: MessageStatus::Answered,
                    error: None,
                    sources: &[source],
                    page_refs: &[MessagePageRef {
                        page: 2,
                        document_id: doc,
                        source: Some(1),
                    }],
                },
            )
            .await
            .unwrap();
        let app = router(AppState {
            system_status: status(LanguageModelStatus::Available, FakeStorage::healthy()),
            ingestion: Ok(ingestion),
            chat: Ok(chat_on(conversations, FakeLlmProvider::available())),
            viewer: Ok(Arc::new(ViewDocument::new(engine, documents))),
            remover: Err("remoção indisponível neste teste".into()),
            diagnostics: None,
            models: None,
            indexing: Err("indexação indisponível neste teste".into()),
            notes: None,
        });
        (app, doc, a)
    }

    #[test]
    fn every_source_has_an_icon_and_opens_its_own_panel() {
        use nlmx_domain::{
            chat::{AnswerGrounding, Message, MessageSource, MessageStatus, Role},
            source::{SourceLocation, SourceReference},
        };
        let source = |n: u32, name: &str, location: SourceLocation| MessageSource {
            n,
            cited: true,
            document_id: n as i64,
            chunk_id: Some(n as i64),
            document_title: format!("Título {n}"),
            page_start: 1,
            page_end: 1,
            section: None,
            label: format!("{name} · {}", location.label()),
            quote: "Trecho citado.".into(),
            bboxes: vec![],
            reference: SourceReference {
                document_id: n as i64,
                document_title: format!("Título {n}"),
                chunk_id: Some(n as i64),
                location,
                section_path: vec![],
            },
            document_name: name.into(),
        };
        let message = Message {
            id: 5,
            conversation_id: 1,
            role: Role::Assistant,
            content: "Está no PDF [1], na tabela [2] e no guia [3].".into(),
            status: MessageStatus::Answered,
            grounding: Some(AnswerGrounding::Documents),
            error: None,
            sources: vec![
                source(
                    1,
                    "arquitetura.pdf",
                    SourceLocation::pdf(12, 12, vec![]).unwrap(),
                ),
                source(2, "dados.csv", SourceLocation::csv(120, 145).unwrap()),
                source(
                    3,
                    "arquitetura.md",
                    SourceLocation::markdown(
                        vec!["Embeddings".into(), "Normalização".into()],
                        None,
                    )
                    .unwrap(),
                ),
            ],
            page_refs: vec![],
            created_at: String::new(),
        };
        let view = chat::answer_view(&message);
        let shown: Vec<(&str, &str, &str, &str, bool)> = view
            .cited
            .iter()
            .map(|s| {
                (
                    s.title.as_str(),
                    s.format,
                    s.icon,
                    s.location.as_str(),
                    s.previewable,
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("arquitetura.pdf", "PDF", "format-pdf", "p. 12", true),
                ("dados.csv", "CSV", "format-csv", "linhas 120–145", false),
                (
                    "arquitetura.md",
                    "Markdown",
                    "format-markdown",
                    "Embeddings › Normalização",
                    false
                ),
            ]
        );
        // Only the PDF opens the viewer; the others open their information panel.
        let urls: Vec<&str> = view.cited.iter().map(|s| s.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "/viewer/1?page=1&cite=5-1",
                "/sources/2?cite=5-2",
                "/sources/3?cite=5-3"
            ]
        );
        assert_eq!(view.html.matches("<button").count(), 3, "{}", view.html);
        assert!(
            view.html
                .contains(r##"hx-get="/sources/2?cite=5-2" hx-target="#viewer""##)
        );
        assert!(!view.html.contains("/viewer/2") && !view.html.contains("/viewer/3"));
        assert!(view.copy_text.contains("[2] dados.csv · linhas 120–145"));
    }

    /// A library with a Markdown file (1), a PDF (2) and a CSV (3); the conversations that cited
    /// the Markdown one are in `conversations`.
    async fn sources_app() -> (Router, i64) {
        sources_app_with(None).await
    }

    /// Records the notes handed to the import queue, validating them like the real one.
    #[derive(Default)]
    struct RecordingNotes(std::sync::Mutex<Vec<String>>);

    impl NoteSubmitter for RecordingNotes {
        fn submit(&self, text: &str) -> Result<(), nlmx_application::ports::NoteSubmitError> {
            let text = nlmx_domain::note::clean_note_text(text)
                .map_err(nlmx_application::ports::NoteSubmitError::Invalid)?;
            self.0.lock().unwrap().push(text);
            Ok(())
        }
    }

    async fn sources_app_with(notes: Option<Arc<dyn NoteSubmitter>>) -> (Router, i64) {
        use nlmx_application::ports::{
            ConversationRepository, DocumentRepository, FinishedAnswer, NewDocument,
        };
        use nlmx_domain::{
            chat::{AnswerGrounding, MessageSource, MessageStatus, Role},
            document_type::DocumentType,
            ingestion::DocumentStatus,
            source::{SourceLocation, SourceReference},
        };
        let documents = Arc::new(FakeDocumentRepository::default());
        for (name, kind, sha) in [
            ("arquitetura.md", DocumentType::Markdown, 'a'),
            ("relatorio.pdf", DocumentType::Pdf, 'b'),
            ("dados.csv", DocumentType::Csv, 'c'),
            ("Arquitetura do novo módulo", DocumentType::Note, 'd'),
        ] {
            let note = kind == DocumentType::Note;
            documents
                .insert(NewDocument {
                    sha256: sha.to_string().repeat(64),
                    original_filename: name.into(),
                    original_path: if note {
                        String::new()
                    } else {
                        format!("/in/{name}")
                    },
                    library_path: if note {
                        String::new()
                    } else {
                        format!("/lib/{name}")
                    },
                    file_size: 2048,
                    document_type: kind,
                    note_text: note.then(|| "Texto da nota.".to_string()),
                })
                .await
                .unwrap();
        }
        documents.force_status(1, DocumentStatus::Indexed);
        documents.force_status(4, DocumentStatus::Indexed);
        documents.force_status(3, DocumentStatus::Failed);
        let conversations = Arc::new(nlmx_testing::FakeConversations::default());
        let c = conversations
            .create(nlmx_domain::chat::ConversationScope::Library)
            .await
            .unwrap();
        let a = conversations
            .add_message(
                c.id,
                Role::Assistant,
                "",
                MessageStatus::Streaming,
                Some(AnswerGrounding::Documents),
            )
            .await
            .unwrap();
        let location =
            SourceLocation::markdown(vec!["Embeddings".into(), "Normalização".into()], None)
                .unwrap();
        conversations
            .finish_message(
                a,
                FinishedAnswer {
                    content: "Há uma camada de normalização [1].",
                    status: MessageStatus::Answered,
                    error: None,
                    sources: &[MessageSource {
                        n: 1,
                        cited: true,
                        document_id: 1,
                        chunk_id: Some(1),
                        document_title: "Arquitetura".into(),
                        page_start: 1,
                        page_end: 1,
                        section: None,
                        label: "arquitetura.md · Embeddings › Normalização".into(),
                        quote: "A camada de normalização vem antes dos embeddings.".into(),
                        bboxes: vec![],
                        reference: SourceReference {
                            document_id: 1,
                            document_title: "Arquitetura".into(),
                            chunk_id: Some(1),
                            location,
                            section_path: vec![],
                        },
                        document_name: "arquitetura.md".into(),
                    }],
                    page_refs: &[],
                },
            )
            .await
            .unwrap();
        let ingestion = Arc::new(DocumentIngestion {
            pipeline: None,
            progress: None,
            viewer: None,
            engine: Arc::new(FakeDocumentEngine::default()),
            files: Arc::new(FakeFileStore::default()),
            documents,
            analyzer: Arc::new(FakeStructureAnalyzer),
            chunker: Arc::new(FakeChunker),
            tokens: Arc::new(WordTokenCounter),
            policy: Default::default(),
            embedder: None,
        });
        let app = router(AppState {
            system_status: status(LanguageModelStatus::Available, FakeStorage::healthy()),
            ingestion: Ok(ingestion),
            chat: Ok(chat_on(conversations, FakeLlmProvider::available())),
            viewer: Err("visualizador indisponível nos testes".into()),
            remover: Err("remoção indisponível neste teste".into()),
            diagnostics: None,
            models: None,
            indexing: Err("indexação indisponível neste teste".into()),
            notes,
        });
        (app, a)
    }

    #[tokio::test]
    async fn a_source_that_is_not_a_pdf_shows_its_information_and_no_preview() {
        let (app, _) = sources_app().await;
        let (status, _, body) = send(app, "/sources/1", true).await;
        assert_eq!(status, StatusCode::OK);
        for expected in [
            "data-source-info",
            "arquitetura.md",
            "Markdown",
            "Indexado",
            "Trechos",
            "Usado em",
            "Ainda não usado em conversas",
            "2 KB",
            "data-close-panel",
        ] {
            assert!(body.contains(expected), "{expected}: {body}");
        }
        // Nothing to preview: no viewer, no PDF button, no content of the file.
        for forbidden in ["data-viewer", "/viewer/", "Abrir no PDF", "viewer-pages"] {
            assert!(!body.contains(forbidden), "{forbidden}: {body}");
        }
    }

    #[tokio::test]
    async fn documents_offer_adding_a_note_next_to_importing() {
        let (app, _) = sources_app_with(Some(Arc::new(RecordingNotes::default()))).await;
        let (status, _, body) = send(app, "/documents", false).await;
        assert_eq!(status, StatusCode::OK);
        for expected in [
            r#"data-command="import_documents""#,
            r#"data-dialog-open="add-note""#,
            "Adicionar nota",
            "Cole o texto copiado",
            "Cole o texto copiado abaixo para enviá-lo como uma fonte.",
            r#"hx-post="/documents/notes""#,
            "<textarea",
            "Cancelar",
            "Inserir",
        ] {
            assert!(body.contains(expected), "{expected}: {body}");
        }
        // "Inserir" starts disabled: there is no text yet.
        assert!(body.contains("data-note-submit disabled"), "{body}");
    }

    #[tokio::test]
    async fn a_pasted_note_goes_to_the_import_queue_and_blank_ones_are_refused() {
        let notes = Arc::new(RecordingNotes::default());
        let (app, _) = sources_app_with(Some(notes.clone())).await;

        let (status, body) = post_form(
            app.clone(),
            "/documents/notes",
            "text=Ol%C3%A1%0D%0A%0D%0Amundo+++",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains(r#"data-toast-on-load="info""#), "{body}");
        assert_eq!(*notes.0.lock().unwrap(), ["Olá\n\nmundo"]);

        for blank in ["text=", "text=+++%0A%09", ""] {
            let (status, body) = post_form(app.clone(), "/documents/notes", blank).await;
            assert_eq!(status, StatusCode::OK);
            assert!(
                body.contains(r#"data-toast-on-load="danger""#)
                    && body.contains("A nota está vazia"),
                "{blank:?}: {body}"
            );
        }
        assert_eq!(
            notes.0.lock().unwrap().len(),
            1,
            "blank notes are never queued"
        );

        // Without an import queue the user is told, not ignored.
        let (without, _) = sources_app_with(None).await;
        let (_, body) = post_form(without, "/documents/notes", "text=a").await;
        assert!(body.contains(r#"data-toast-on-load="danger""#), "{body}");
    }

    #[tokio::test]
    async fn a_note_is_a_normal_source_without_a_viewer_file_name_or_size() {
        let (app, _) = sources_app().await;
        let (_, _, list) = send(app.clone(), "/fragments/documents", true).await;
        assert!(
            list.contains("Arquitetura do novo módulo") && list.contains("Nota"),
            "{list}"
        );
        assert!(list.contains(r#"hx-get="/chat?source=4""#), "{list}");
        assert!(
            !list.contains("/chat?view=4") && !list.contains("/viewer/4"),
            "a note has no viewer: {list}"
        );

        let (status, _, panel) = send(app, "/sources/4", true).await;
        assert_eq!(status, StatusCode::OK);
        for expected in ["Arquitetura do novo módulo", "Nota", "Indexado", "Trechos"] {
            assert!(panel.contains(expected), "{expected}: {panel}");
        }
        for forbidden in [
            "Abrir no PDF",
            "/viewer/",
            "data-viewer",
            "<dt>Arquivo</dt>",
            "<dt>Tamanho</dt>",
        ] {
            assert!(!panel.contains(forbidden), "{forbidden}: {panel}");
        }
    }

    #[tokio::test]
    async fn opening_from_a_citation_shows_where_the_answer_came_from() {
        let (app, answer) = sources_app().await;
        let (status, _, body) = send(app, &format!("/sources/1?cite={answer}-1"), true).await;
        assert_eq!(status, StatusCode::OK);
        for expected in [
            "Fonte 1",
            "Trecho citado · Embeddings › Normalização",
            "A camada de normalização vem antes dos embeddings.",
        ] {
            assert!(body.contains(expected), "{expected}: {body}");
        }
        assert!(!body.contains("/viewer/"));
    }

    #[tokio::test]
    async fn a_pdf_source_also_offers_its_preview() {
        let (app, _) = sources_app().await;
        let (status, _, body) = send(app, "/sources/2", true).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.contains(r##"hx-get="/viewer/2" hx-target="#viewer""##),
            "{body}"
        );
        assert!(body.contains("PDF") && body.contains("Abrir no PDF"));
    }

    #[tokio::test]
    async fn a_failed_source_says_so_and_a_missing_one_is_not_found() {
        let (app, _) = sources_app().await;
        let (_, _, failed) = send(app.clone(), "/sources/3", true).await;
        assert!(
            failed.contains("CSV") && failed.contains("Falhou"),
            "{failed}"
        );
        let (status, _, body) = send(app, "/sources/99", true).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("viewer-error") && body.contains("data-close-panel"));
    }

    #[tokio::test]
    async fn documents_show_each_format_and_only_non_pdfs_have_details() {
        let (app, _) = sources_app().await;
        let (_, _, body) = send(app.clone(), "/fragments/documents", true).await;
        for expected in ["arquitetura.md", "Markdown", "CSV", "PDF"] {
            assert!(body.contains(expected), "{expected}: {body}");
        }
        assert!(body.contains(r#"hx-get="/chat?source=1""#), "{body}");
        assert!(body.contains(r#"hx-get="/chat?source=3""#));
        assert!(
            !body.contains("/chat?source=2"),
            "a PDF opens its viewer instead"
        );
        // A page of a file that has none is not invented.
        assert!(!body.contains("0 páginas"));

        // The chat opens with the information panel beside it.
        let (status, _, chat) = send(app, "/chat?source=1", false).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            chat.contains("data-source-info") && chat.contains(" data-panel-open"),
            "{chat}"
        );
    }

    #[tokio::test]
    async fn citations_open_the_viewer_at_the_page_with_highlights() {
        let (app, doc, a) = viewer_app().await;
        // The answer links its source and its page reference to the viewer.
        let (_, _, answer) = send(app.clone(), &format!("/chat/messages/{a}"), true).await;
        assert!(
            answer.contains("Abrir no PDF"),
            "sources show an explicit open action"
        );
        assert!(
            answer.contains(&format!(
                r##"hx-get="/viewer/{doc}?page=1&amp;cite={a}-1" hx-target="#viewer""##
            )),
            "{answer}"
        );
        assert!(
            answer.contains(&format!(
                r##"class="page-ref" hx-get="/viewer/{doc}?page=2&amp;ref={a}-2""##
            )),
            "{answer}"
        );

        let (status, _, body) = send(
            app.clone(),
            &format!("/viewer/{doc}?page=1&cite={a}-1"),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.contains(r#"data-target-page="1""#) && body.contains(r#"data-page-count="2""#)
        );
        assert!(body.contains("Fonte 1"), "origin shown");
        assert!(
            body.contains(r#"data-anchor="10""#),
            "positioned at the passage: {body}"
        );
        // Every page is laid out with its size; highlights on both pages, in % (612×792 pt).
        assert_eq!(body.matches(r#"class="viewer-page""#).count(), 2);
        assert!(body.contains(r#"style="--page-w: 612; --page-h: 792""#));
        assert!(body.contains("left: 10%; top: 10%; width: 40%; height: 10%"));
        assert!(body.contains("left: 10%; top: 50%; width: 40%; height: 10%"));
        assert_eq!(
            body.matches(r#"class="viewer-thumb""#).count(),
            2,
            "thumbnails"
        );
        // No ids inside the viewer: HTMX 4 copies an old element's attributes onto a new one
        // with the same id while settling, which broke re-opening (blank pages).
        let viewer_html = &body[body.find("data-viewer ").unwrap()..];
        assert!(!viewer_html.contains(" id="), "{viewer_html}");
        // Versioned by the file hash: an id reused after a removal never hits a cached image.
        assert!(body.contains(&format!(
            r#"data-src-base="/documents/{doc}/pages/2.png?v="#
        )));
        assert!(body.contains(&format!(r#"data-text-src="/viewer/{doc}/pages/1/text""#)));

        // A page reference opens that page with the source's boxes on it only.
        let (_, _, body) = send(
            app.clone(),
            &format!("/viewer/{doc}?page=2&ref={a}-2"),
            true,
        )
        .await;
        assert!(body.contains(r#"data-target-page="2""#) && body.contains("Página 2"));
        assert_eq!(body.matches(r#"class="viewer-highlight""#).count(), 1);
        assert!(body.contains(r#"data-anchor="50""#));

        // Without a citation: no highlights, page 1, out-of-range pages clamped.
        let (_, _, plain) = send(app.clone(), &format!("/viewer/{doc}?page=9"), true).await;
        assert!(plain.contains(r#"data-target-page="2""#) && !plain.contains("viewer-highlight\""));
        let (status, _, _) = send(app, "/viewer/99", true).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn text_layer_search_and_page_images() {
        let (app, doc, _) = viewer_app().await;
        let (status, _, text) =
            send(app.clone(), &format!("/viewer/{doc}/pages/1/text"), true).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            text.contains("Introdução &#60;b&#62;sobre&#60;/b&#62; a carência")
                || text.contains("Introdução &lt;b&gt;sobre&lt;/b&gt; a carência"),
            "escaped: {text}"
        );
        assert!(text.contains("left: 10%; top: 10%"));

        let (status, headers, json) = send(
            app.clone(),
            &format!("/viewer/{doc}/search?q=CARENCIA"),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[http::header::CONTENT_TYPE], "application/json");
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let pages: Vec<u64> = v["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["page"].as_u64().unwrap())
            .collect();
        assert_eq!(pages, [1, 2]);
        assert_eq!(v["truncated"], false);

        let (status, headers, _) = send(
            app.clone(),
            &format!("/documents/{doc}/pages/1.png?w=600"),
            false,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[http::header::CONTENT_TYPE], "image/png");
        let (status, _, _) =
            send(app.clone(), &format!("/documents/{doc}/pages/9.png"), false).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // The chat page can open with a document in the viewer (Documentos › Abrir).
        let (_, _, page) = send(app.clone(), &format!("/chat?view={doc}"), true).await;
        assert!(
            page.contains("data-panel-open") && page.contains("data-viewer "),
            "{page}"
        );
        let (_, _, docs) = send(app, "/documents", true).await;
        assert!(
            docs.contains(&format!(r#"hx-get="/chat?view={doc}""#)),
            "{docs}"
        );
    }

    #[tokio::test]
    async fn conversations_can_be_created_scoped_and_deleted() {
        let chat = chat_service(FakeLlmProvider::available());
        let app = chat_app(chat.clone(), Err("x".into())).await;
        let (_, first) = post_form(app.clone(), "/chat/new", "").await;
        assert!(first.contains(r#"data-conversation="1""#), "{first}");
        assert_eq!(
            chat.conversation(1).await.unwrap().scope,
            ConversationScope::Free
        );
        let (_, scoped) = post_form(app.clone(), "/chat/1/scope", "scope=1").await;
        assert!(
            scoped.contains(r#"<option value="1" selected>"#),
            "{scoped}"
        );
        assert!(scoped.contains(r#"<input type="hidden" name="scope" value="1">"#));
        assert_eq!(
            chat.conversation(1).await.unwrap().scope,
            ConversationScope::Document(1)
        );
        let (_, second) = post_form(app.clone(), "/chat/new", "scope=1").await;
        assert!(second.contains(r#"data-conversation="2""#));
        assert_eq!(
            chat.conversation(2).await.unwrap().scope,
            ConversationScope::Document(1),
            "new keeps the scope"
        );
        post_form(app.clone(), "/chat/2/scope", "scope=free").await;
        assert_eq!(
            chat.conversation(2).await.unwrap().scope,
            ConversationScope::Free
        );
        let (_, after) = post_form(app, "/chat/2/delete", "").await;
        assert!(!after.contains(r#"data-conversation="2""#));
    }

    #[tokio::test]
    async fn models_page_offers_confirmed_downloads_and_shows_installed_models() {
        use nlmx_application::ports::{CancelFlag, ModelProvider};
        let models = Arc::new(nlmx_testing::FakeModelProvider::new(vec![
            descriptor("a"),
            descriptor("b"),
        ]));
        let plan = models.plan_download("b").await.unwrap();
        models
            .download(plan.confirm(), Arc::new(|_| {}), CancelFlag::default())
            .await
            .unwrap();
        models.activate("b").await.unwrap();

        let app = router(AppState {
            system_status: status(LanguageModelStatus::Available, FakeStorage::healthy()),
            ingestion: Err("x".into()),
            chat: Ok(chat_service(FakeLlmProvider::available())),
            viewer: Err("x".into()),
            remover: Err("remoção indisponível neste teste".into()),
            diagnostics: None,
            models: Some(models),
            indexing: Err("indexação indisponível neste teste".into()),
            notes: None,
        });
        let (_, _, body) = send(app.clone(), "/models", true).await;
        assert!(body.contains("Modelo a") && body.contains("Não instalado"));
        assert!(
            body.contains(r#"data-dialog-open="plan-a""#),
            "download goes through a confirmation dialog"
        );
        assert!(
            body.contains("Baixar 639 MB")
                && body.contains("Apache-2.0")
                && body.contains("Espaço necessário")
        );
        assert!(body.contains("Em uso"), "the active model");
        assert!(
            !body.contains(r#"data-dialog-open="plan-b""#),
            "installed models are not offered again"
        );
        assert!(body.contains("llama.cpp b11349"));
        let (_, _, fragment) = send(app, "/fragments/models", true).await;
        assert!(fragment.contains("Modelo b") && !fragment.contains("page-header"));
    }

    #[tokio::test]
    async fn assets_are_served_with_content_types() {
        for (path, content_type) in [
            ("/assets/app.css", "text/css; charset=utf-8"),
            ("/assets/htmx.min.js", "text/javascript; charset=utf-8"),
            ("/assets/ds.js", "text/javascript; charset=utf-8"),
            ("/assets/app.js", "text/javascript; charset=utf-8"),
            ("/assets/viewer.js", "text/javascript; charset=utf-8"),
        ] {
            let (status, headers, body) = get(path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert_eq!(headers[http::header::CONTENT_TYPE], content_type, "{path}");
            assert!(!body.is_empty(), "{path}");
        }
    }

    #[cfg(debug_assertions)]
    #[tokio::test]
    async fn design_system_gallery_is_available_in_debug_builds() {
        let (status, _, body) = get("/design-system?theme=dark").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains(r#"data-theme="dark""#));
        assert!(body.contains("--color-accent"));
    }
}
