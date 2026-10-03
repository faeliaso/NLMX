//! The whole app minus the native shell: real PDFium + SQLite + structure/chunker, the UI router
//! (as the `nlmx://` protocol drives it), the chat flow (as the `answer_message` command runs
//! it), the PDF viewer, and the local metrics collected along the way. The language model and the
//! embedder are deterministic fakes; `make test-real` covers the real ones.

mod support;

use std::sync::Arc;

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use nlmx_application::{
    ports::CancelFlag,
    services::{rag::RagOptions, retriever::RetrieverOptions},
    use_cases::GetSystemStatus,
};
use nlmx_domain::{ingestion::ImportOutcome, telemetry::Operation};
use nlmx_telemetry::{MetricsLayer, MetricsRegistry};
use nlmx_testing::{FakeLlmProvider, FakeRuntime};
use nlmx_ui_web::{AppState, router};
use support::{Library, deterministic_embeddings, fixture, temp_dir};
use tower::ServiceExt;
use tracing_subscriber::layer::SubscriberExt;

async fn get(app: &axum::Router, path: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::get(path)
                .header("HX-Request", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn post(app: &axum::Router, path: &str, form: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::post(path)
                .header("HX-Request", "true")
                .header(
                    http::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(Body::from(form.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

fn attr<'a>(html: &'a str, after: &str, name: &str) -> &'a str {
    let from = &html[html
        .find(after)
        .unwrap_or_else(|| panic!("{after} in {html}"))..];
    let start = from.find(&format!("{name}=\"")).unwrap() + name.len() + 2;
    &from[start..start + from[start..].find('"').unwrap()]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_ask_cite_open_and_measure() {
    let metrics = MetricsRegistry::new();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(MetricsLayer::new(metrics.clone())),
    )
    .unwrap();

    // 1. Import: two valid PDFs and a password-protected one (a measured failure).
    let mut library = Library::new(temp_dir("app-e2e"), deterministic_embeddings());
    library.import(&["report.pdf", "text.pdf"]).await;
    let failed = library.ingestion.import(&fixture("encrypted.pdf")).await;
    assert!(
        matches!(&failed, ImportOutcome::Failed { reason, .. } if reason.contains("senha")),
        "{failed:?}"
    );

    let llm =
        Arc::new(FakeLlmProvider::available().answering(
            "O prazo de carência termina após cento e oitenta dias [1], ver [página 3].",
        ));
    let chat = Arc::new(library.chat(
        llm.clone(),
        RagOptions {
            retriever: RetrieverOptions {
                min_score: 0.0,
                ..Default::default()
            },
            min_relevance: 0.1,
            ..Default::default()
        },
    ));
    let app = router(AppState {
        system_status: Arc::new(GetSystemStatus::new(
            llm.clone(),
            library.db.clone(),
            Ok("chromium/7881".into()),
            Arc::new(FakeRuntime::llama()),
        )),
        ingestion: Ok(Arc::new(nlmx_application::use_cases::DocumentIngestion {
            engine: library.ingestion.engine.clone(),
            files: library.ingestion.files.clone(),
            documents: library.ingestion.documents.clone(),
            analyzer: library.ingestion.analyzer.clone(),
            chunker: library.ingestion.chunker.clone(),
            tokens: library.ingestion.tokens.clone(),
            policy: library.ingestion.policy,
            embedder: library.ingestion.embedder.clone(),
        })),
        chat: Ok(chat.clone()),
        viewer: Ok(Arc::new(library.viewer())),
        diagnostics: None,
        models: None,
    });

    // 2. The library lists the documents (and the failure).
    let (status, docs) = get(&app, "/documents").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        docs.contains("Relatório de Coberturas") && docs.contains("Contrato de Exemplo"),
        "{docs}"
    );
    assert!(
        docs.contains("protegido por senha"),
        "the failed import shows its reason"
    );

    // 3. Ask in the chat: the turn comes back streaming…
    let (_, page) = get(&app, "/chat").await;
    let conversation = attr(&page, "data-chat-page", "data-conversation").to_string();
    let (status, turn) = post(
        &app,
        &format!("/chat/{conversation}/messages"),
        "q=Quando+termina+o+prazo+de+car%C3%AAncia%3F",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let answer_id: i64 = attr(&turn, "data-chat-turn", "data-answer")
        .parse()
        .unwrap();
    assert!(turn.contains(r#"data-status="streaming""#));

    // 4. …the command generates it (tokens streamed, answer saved)…
    let tokens = Arc::new(std::sync::Mutex::new(String::new()));
    let sink = tokens.clone();
    chat.answer(
        answer_id,
        &move |t| sink.lock().unwrap().push_str(t),
        CancelFlag::default(),
    )
    .await
    .unwrap();
    assert!(tokens.lock().unwrap().contains("cento e oitenta dias"));

    // 5. …and the final fragment has a clickable citation and a page reference.
    let (_, answer) = get(&app, &format!("/chat/messages/{answer_id}")).await;
    assert!(answer.contains(r#"data-status="answered""#), "{answer}");
    let citation = attr(&answer, r#"class="citation""#, "hx-get").replace("&amp;", "&");
    assert!(citation.starts_with("/viewer/"), "{citation}");
    assert!(
        answer.contains(r#"class="page-ref""#),
        "[página 3] is a link"
    );

    // 6. The citation opens the viewer at the page, positioned and highlighted.
    let (status, viewer) = get(&app, &citation).await;
    assert_eq!(status, StatusCode::OK);
    assert!(viewer.contains("data-viewer ") && viewer.contains("Fonte 1"));
    assert!(viewer.contains("viewer-highlight"), "highlight boxes");
    assert!(
        !attr(&viewer, "data-viewer ", "data-anchor").is_empty(),
        "positioned on the passage"
    );
    let document: i64 = attr(&viewer, "data-viewer ", "data-document")
        .parse()
        .unwrap();

    // 7. Search, text layer and page image of the document.
    let (_, json) = get(&app, &format!("/viewer/{document}/search?q=car%C3%AAncia")).await;
    let hits: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(!hits["hits"].as_array().unwrap().is_empty(), "{json}");
    let (status, text) = get(&app, &format!("/viewer/{document}/pages/1/text")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(text.contains("<span"));
    let (status, _) = get(&app, &format!("/documents/{document}/pages/1.png?w=600")).await;
    assert_eq!(status, StatusCode::OK);

    // 8. Measurements: imports (with the failure), embeddings, retrieval and generation.
    let s = metrics.snapshot();
    assert_eq!(s.documents_imported, 2);
    assert!(
        s.pages >= 5 && s.chunks > 0 && s.pages_per_second.is_some(),
        "{s:?}"
    );
    let ingest = s.operation(Operation::Ingest).unwrap();
    assert_eq!((ingest.total, ingest.failed), (3, 1));
    assert!((ingest.error_rate().unwrap() - 1.0 / 3.0).abs() < 1e-9);
    assert!(s.embeddings > 0 && s.embeddings_per_second.is_some());
    assert!(s.operation(Operation::Retrieve).unwrap().total >= 1);
    let generate = s.operation(Operation::Generate).unwrap();
    assert_eq!((generate.total, generate.failed), (1, 0));
    assert!(s.first_token_p50_ms.is_some());
}
