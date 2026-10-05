//! The interface over the real stack for the five formats: the router the app serves, with real
//! PDFium and SQLite behind it. A source is shown with its icon, name, format and status; only a
//! PDF has a preview; clicking a PDF source opens the viewer at its page and clicking any other
//! opens its information — never a viewer.

mod support;

use std::sync::Arc;

use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use nlmx_application::{
    ports::CancelFlag,
    services::{
        free_chat::FreeChat,
        rag::{RagEngine, RagOptions},
        retrieval::HybridRetriever,
        retriever::{Retriever, RetrieverOptions},
    },
    use_cases::{ChatService, GetSystemStatus, Indexing, IndexingActivity, ViewDocument},
};
use nlmx_domain::chat::ConversationScope;
use nlmx_testing::{FakeLlmProvider, FakeRuntime};
use nlmx_ui_web::{AppState, router};
use support::{
    fixture,
    multiformat::{App, epub_fixture, imported, office_fixture},
    root, shared_engine,
};
use tower::ServiceExt;

/// A fragment of each format's icon (its drawing), to tell the icons apart in the HTML.
const ICONS: [(&str, &str); 7] = [
    ("report.pdf", "M8.5 18v-5h1.75"),
    ("manual.md", "M8 18v-5l2 2.5"),
    ("reuniao.txt", "M9 13h6M9 16h6M9 19h3"),
    ("vendas.csv", "M8 12.5h8v6H8z"),
    ("livro.epub", "M4 19.5V5a2 2 0 0 1 2-2h13v15H6"),
    ("contrato.docx", "M14 3H7a2 2 0 0 0-2 2v14"),
    ("vendas.xlsx", "M4 10h16"),
];
const FORMATS: [(&str, &str); 7] = [
    ("report.pdf", "PDF"),
    ("manual.md", "Markdown"),
    ("reuniao.txt", "TXT"),
    ("vendas.csv", "CSV"),
    ("livro.epub", "EPUB"),
    ("contrato.docx", "DOCX"),
    ("vendas.xlsx", "XLSX"),
];

async fn request(app: &axum::Router, request: Request<Body>) -> (StatusCode, String) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn get(app: &axum::Router, path: &str) -> (StatusCode, String) {
    request(
        app,
        Request::get(path)
            .header("HX-Request", "true")
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn post(app: &axum::Router, path: &str, form: &str) -> (StatusCode, String) {
    request(
        app,
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
}

fn attr<'a>(html: &'a str, after: &str, name: &str) -> &'a str {
    let from = &html[html
        .find(after)
        .unwrap_or_else(|| panic!("{after} in {html}"))..];
    let start = from.find(&format!("{name}=\"")).unwrap() + name.len() + 2;
    &from[start..start + from[start..].find('"').unwrap()]
}

/// The `<li>` of a list (a row of Documentos or Indexação, an item of Fontes) that mentions `name`.
fn item<'a>(html: &'a str, marker: &str, name: &str) -> &'a str {
    html.split(marker)
        .skip(1)
        .find(|chunk| chunk.contains(name))
        .map(|chunk| chunk.split("</li>").next().unwrap())
        .unwrap_or_else(|| panic!("no {marker} item for {name} in {html}"))
}

struct Setup {
    app: axum::Router,
    ids: Vec<(&'static str, i64)>,
    chat: Arc<ChatService>,
}

async fn setup(name: &str) -> Setup {
    let app = App::new(name, true);
    let inbox = app.dir.join("inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    let sources = [
        ("report.pdf", fixture("report.pdf")),
        ("manual.md", root().join("tests/golden/corpus/manual.md")),
        (
            "reuniao.txt",
            root().join("tests/golden/corpus/reuniao.txt"),
        ),
        ("vendas.csv", root().join("tests/golden/corpus/vendas.csv")),
        ("livro.epub", epub_fixture("livro.epub")),
        ("contrato.docx", office_fixture("contrato.docx")),
        ("vendas.xlsx", office_fixture("vendas.xlsx")),
    ];
    let mut ids = Vec::new();
    for (file, source) in &sources {
        let path = app.user_file(source, file);
        ids.push((*file, imported(app.ingestion.import(&path).await).0));
    }

    let llm = Arc::new(
        FakeLlmProvider::available()
            .answering("A carência aparece nos documentos [1][2][3][4][5][6][7][8][9]."),
    );
    let retriever = Arc::new(Retriever::new(Arc::new(HybridRetriever::new(
        app.model.clone(),
        app.db.clone(),
        app.db.clone(),
        app.db.clone(),
    ))));
    let chat = Arc::new(ChatService {
        conversations: app.db.clone(),
        rag: Arc::new(RagEngine::new(retriever, app.db.clone(), llm.clone())),
        free: Arc::new(FreeChat::new(llm.clone())),
        options: RagOptions {
            retriever: RetrieverOptions {
                top_k: 12,
                max_per_document: 4,
                min_score: 0.0,
                ..Default::default()
            },
            min_relevance: 0.0,
            ..Default::default()
        },
    });
    let App {
        db,
        ingestion,
        embedder,
        model,
        ..
    } = app;
    let ingestion = Arc::new(ingestion);
    let indexing = Arc::new(Indexing {
        reader: db.clone(),
        documents: db.clone(),
        embeddings: model,
        embedder,
        ingestion: Ok(ingestion.clone()),
        activity: Arc::new(IndexingActivity::default()),
    });
    let router = router(AppState {
        system_status: Arc::new(GetSystemStatus::new(
            llm,
            db.clone(),
            Ok("chromium/7881".into()),
            Arc::new(FakeRuntime::llama()),
        )),
        ingestion: Ok(ingestion),
        chat: Ok(chat.clone()),
        viewer: Ok(Arc::new(ViewDocument::new(shared_engine(), db.clone()))),
        remover: Err("remoção indisponível neste teste".into()),
        diagnostics: None,
        models: None,
        indexing: Ok(indexing),
    });
    Setup {
        app: router,
        ids,
        chat,
    }
}

fn id_of(setup: &Setup, file: &str) -> i64 {
    setup.ids.iter().find(|(f, _)| *f == file).unwrap().1
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sources_show_icon_name_format_and_status_in_every_list() {
    let s = setup("fe-lists").await;

    // Documentos: icon, file name, format and status of each source; only the PDF opens a viewer.
    let (status, docs) = get(&s.app, "/fragments/documents").await;
    assert_eq!(status, StatusCode::OK);
    for ((file, icon), (_, format)) in ICONS.iter().zip(FORMATS) {
        let row = item(&docs, r#"<li class="list-row">"#, file);
        assert!(row.contains(icon), "{file}: icon");
        assert!(row.contains(format), "{file}: format {format}");
        assert!(
            row.contains("Aguardando embeddings") || row.contains("Indexado"),
            "{file}: status"
        );
        let id = id_of(&s, file);
        if *file == "report.pdf" {
            assert!(
                row.contains(&format!(r#"hx-get="/chat?view={id}""#)),
                "{file}: opens the viewer"
            );
            assert!(!row.contains("Detalhes"));
        } else {
            assert!(
                row.contains(&format!(r#"hx-get="/chat?source={id}""#)),
                "{file}: details"
            );
            assert!(!row.contains("?view="), "{file}: no viewer");
        }
    }
    // The page counts only for a paged format.
    assert!(item(&docs, r#"<li class="list-row">"#, "report.pdf").contains("página"));
    assert!(!item(&docs, r#"<li class="list-row">"#, "vendas.csv").contains("página"));

    // Indexação: the same icon and format on every row.
    let (status, indexing) = get(&s.app, "/indexing").await;
    assert_eq!(status, StatusCode::OK);
    // (the rows carry the document's title, which is not always the file name). "Concluídos
    // recentemente" lists the five last indexed, so the first documents are not there.
    for ((file, icon), (_, format)) in ICONS.iter().zip(FORMATS).skip(ICONS.len() - 5) {
        assert!(indexing.contains(icon), "{file}: icon");
        assert!(
            indexing.contains(&format!(r#"list-row-meta tabular">{format}"#)),
            "{file}: format {format}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn citations_and_sources_open_the_right_panel_and_never_a_viewer_for_non_pdfs() {
    let s = setup("fe-citations").await;

    // A conversation about every document, asked and answered as the app does.
    let c = s.chat.start(ConversationScope::Library).await.unwrap();
    let (_, form) = (0, "q=Onde+se+fala+de+car%C3%AAncia%3F");
    let (status, turn) = post(&s.app, &format!("/chat/{}/messages", c.id), form).await;
    assert_eq!(status, StatusCode::OK);
    let answer_id: i64 = attr(&turn, "data-chat-turn", "data-answer")
        .parse()
        .unwrap();
    s.chat
        .answer(answer_id, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    let (_, answer) = get(&s.app, &format!("/chat/messages/{answer_id}")).await;

    // Sources: icon, name, format, location; PDF → viewer URL, others → source information.
    let mut urls = Vec::new();
    for ((file, icon), (_, format)) in ICONS.iter().zip(FORMATS) {
        let entry = item(&answer, r#"class="source-item""#, file);
        assert!(entry.contains(icon), "{file}: icon");
        assert!(
            entry.contains(&format!(">{format}</span>")),
            "{file}: format"
        );
        let url = attr(entry, "hx-get", "hx-get")
            .replace("&amp;", "&")
            .replace("&#38;", "&");
        if *file == "report.pdf" {
            assert!(url.starts_with("/viewer/"), "{file}: {url}");
            assert!(entry.contains("Abrir no PDF"));
        } else {
            assert!(url.starts_with("/sources/"), "{file}: {url}");
            assert!(
                entry.contains("Ver fonte") && !entry.contains("Abrir no PDF"),
                "{file}"
            );
        }
        urls.push((*file, url));
    }
    // The [n] markers in the text are buttons into the same panels.
    assert!(
        answer
            .matches(r#"class="citation" hx-get="/viewer/"#)
            .count()
            >= 1
    );
    assert!(
        answer
            .matches(r#"class="citation" hx-get="/sources/"#)
            .count()
            >= 4
    );
    assert!(
        !answer.contains(r#"<span class="citation" role="note""#),
        "none is inert"
    );

    // Clicking: the PDF opens the viewer at the page; the others show their information.
    for (file, url) in &urls {
        let (status, panel) = get(&s.app, url).await;
        assert_eq!(status, StatusCode::OK, "{file}: {url}");
        if *file == "report.pdf" {
            assert!(
                panel.contains("data-viewer ") && panel.contains("viewer-pages"),
                "{file}"
            );
            eprintln!(
                "URL {url}
{}",
                &panel[..panel.len().min(900)]
            );
            assert!(
                panel.contains("Fonte "),
                "{file}: positioned by the citation"
            );
        } else {
            assert!(panel.contains("data-source-info"), "{file}");
            assert!(
                panel.contains("Informações da fonte") && panel.contains("Trecho citado"),
                "{file}"
            );
            assert!(
                panel.contains("Indexado") || panel.contains("Aguardando embeddings"),
                "{file}"
            );
            assert!(panel.contains("trecho"), "{file}: chunk count");
            for forbidden in [
                "data-viewer",
                "viewer-pages",
                "viewer-toolbar",
                "/viewer/",
                "Abrir no PDF",
            ] {
                assert!(!panel.contains(forbidden), "{file}: {forbidden}");
            }
        }
    }

    // There is no viewer for a source that is not a PDF, whichever way it is asked for.
    for (file, _) in &ICONS[1..] {
        let id = id_of(&s, file);
        for path in [
            format!("/viewer/{id}"),
            format!("/viewer/{id}?page=1"),
            format!("/viewer/{id}/pages/1/text"),
            format!("/viewer/{id}/search?q=carencia"),
            format!("/documents/{id}/pages/1.png?w=800"),
        ] {
            let (status, body) = get(&s.app, &path).await;
            assert_ne!(status, StatusCode::OK, "{file}: {path}\n{body}");
            assert!(!body.contains("data-viewer"), "{file}: {path}");
        }
        // Opening the chat on it shows the information, not a broken page.
        let (status, page) = get(&s.app, &format!("/chat?view={id}")).await;
        assert!(!page.contains("data-viewer "), "{file}: {status}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_pdf_viewer_still_works_on_a_pdf_indexed_by_the_app_pipeline() {
    let s = setup("fe-pdf").await;
    let pdf = id_of(&s, "report.pdf");
    let (status, viewer) = get(&s.app, &format!("/viewer/{pdf}?page=2")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(viewer.contains("data-viewer ") && viewer.contains(r#"data-target-page="2""#));
    assert!(viewer.contains("viewer-pages"));
    let (status, text) = get(&s.app, &format!("/viewer/{pdf}/pages/1/text")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(text.contains("<span"), "{text}");
    let (_, json) = get(&s.app, &format!("/viewer/{pdf}/search?q=car%C3%AAncia")).await;
    let hits: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(!hits["hits"].as_array().unwrap().is_empty(), "{json}");
    // The page image is a PNG.
    let response = s
        .app
        .clone()
        .oneshot(
            Request::get(format!("/documents/{pdf}/pages/1.png?w=600"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..4], b"\x89PNG");

    // Opening the chat on the PDF shows the viewer; on a CSV, its information.
    let (_, page) = get(&s.app, &format!("/chat?view={pdf}")).await;
    assert!(page.contains("data-viewer "));
    let csv = id_of(&s, "vendas.csv");
    let (_, page) = get(&s.app, &format!("/chat?source={csv}")).await;
    assert!(page.contains("data-source-info") && !page.contains("data-viewer "));
}
