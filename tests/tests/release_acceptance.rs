//! Release acceptance (`make acceptance`, after `make bundle`): the **installed app's runtime**
//! (PDFium, llama-server and its libraries from inside `NLMX.app`) with a fresh data directory:
//! model download through the Model Manager (confirmed, checksum), then — offline, verified with
//! `lsof` — ingestion, RAG with Apple Foundation Models, chat, citations and the PDF viewer.
//! Writes `target/acceptance/report.json` for docs/releases/<version>.md.

mod support;

use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use axum::body::Body;
use http::Request;
use http_body_util::BodyExt;
use nlmx_application::{
    ports::{CancelFlag, EmbeddingProvider, LlmProvider, ModelProvider},
    services::rag::{AnswerStatus, RagOptions},
    use_cases::GetSystemStatus,
};
use nlmx_domain::{chat::MessageStatus, ingestion::ImportOutcome, telemetry::Operation};
use nlmx_telemetry::{DataLayout, MetricsLayer, MetricsRegistry, snapshot_json};
use nlmx_testing::FixedEmbeddingSource;
use serde_json::{Value, json};
use support::{Library, fixture, root, temp_dir};
use tower::ServiceExt;
use tracing_subscriber::layer::SubscriberExt;

const MODEL: &str = "qwen3-embedding-0.6b-q8_0";

/// PIDs of this process and all its descendants (llama-server, fm serve…).
fn process_tree() -> Vec<u32> {
    let mut pids = vec![std::process::id()];
    let mut i = 0;
    while i < pids.len() {
        if let Ok(out) = std::process::Command::new("/usr/bin/pgrep")
            .args(["-P", &pids[i].to_string()])
            .output()
        {
            pids.extend(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter_map(|l| l.trim().parse::<u32>().ok()),
            );
        }
        i += 1;
    }
    pids
}

/// Records every remote endpoint of internet sockets opened by the process tree.
struct NetworkWatch {
    stop: Arc<AtomicBool>,
    remotes: Arc<Mutex<BTreeSet<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl NetworkWatch {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let remotes = Arc::new(Mutex::new(BTreeSet::new()));
        let (s, r) = (stop.clone(), remotes.clone());
        let thread = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                let pids: Vec<String> = process_tree().iter().map(u32::to_string).collect();
                if let Ok(out) = std::process::Command::new("/usr/sbin/lsof")
                    .args(["-nP", "-a", "-i", "-p", &pids.join(",")])
                    .output()
                {
                    for line in String::from_utf8_lossy(&out.stdout).lines().skip(1) {
                        if let Some((_, rest)) = line.split_once("->") {
                            let mut parts = rest.split_whitespace();
                            let remote = parts.next().unwrap_or_default();
                            let state = parts
                                .next()
                                .unwrap_or("")
                                .trim_matches(|c| c == '(' || c == ')');
                            r.lock().unwrap().insert(format!("{remote} {state}"));
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        });
        Self {
            stop,
            remotes,
            thread: Some(thread),
        }
    }

    /// Remote endpoints that are not on this machine and were active (not closing).
    fn finish(mut self) -> (Vec<String>, Vec<String>) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let all: Vec<String> = self.remotes.lock().unwrap().iter().cloned().collect();
        let external = all
            .iter()
            .filter(|r| {
                !(r.starts_with("127.0.0.1:")
                    || r.starts_with("[::1]:")
                    || r.starts_with("localhost:"))
            })
            .filter(|r| r.ends_with(" ESTABLISHED") || r.ends_with(" SYN_SENT"))
            .cloned()
            .collect();
        (all, external)
    }
}

async fn get(app: &axum::Router, path: &str) -> (u16, String) {
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
    let status = response.status().as_u16();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

fn attr<'a>(html: &'a str, after: &str, name: &str) -> &'a str {
    let from = &html[html.find(after).unwrap_or_else(|| panic!("{after}"))..];
    let start = from.find(&format!("{name}=\"")).unwrap() + name.len() + 2;
    &from[start..start + from[start..].find('"').unwrap()]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "installed app + model download + Apple FM (make acceptance)"]
async fn installed_app_end_to_end() {
    // The runtime must come from the installed app, not from runtime/.
    let app_path = std::env::var("ACCEPTANCE_APP").expect("ACCEPTANCE_APP=<…/NLMX.app>");
    for var in ["NLMX_PDFIUM_PATH", "NLMX_LLAMA_SERVER"] {
        let value = std::env::var(var).unwrap_or_default();
        assert!(
            value.starts_with(&app_path),
            "{var}={value} must point inside the installed app"
        );
    }
    let metrics = MetricsRegistry::new();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(MetricsLayer::new(metrics.clone())),
    )
    .unwrap();
    let mut report = json!({ "app": app_path });
    let data = temp_dir("acceptance").join("Application Support/dev.nlmx.desktop");
    std::fs::create_dir_all(&data).unwrap();

    // ── 1. Model download through the Model Manager (the user's confirmation = confirm()) ──
    let models = nlmx_models_catalog::LocalModelProvider::in_data_dir(&data);
    let plan = models.plan_download(MODEL).await.unwrap();
    let progress_events = Arc::new(Mutex::new(0usize));
    let counter = progress_events.clone();
    let started = Instant::now();
    let installed = models
        .download(
            plan.confirm(),
            Arc::new(move |_| *counter.lock().unwrap() += 1),
            CancelFlag::default(),
        )
        .await
        .expect("model download");
    let download_secs = started.elapsed().as_secs_f64();
    assert_eq!(installed.size, 639_150_592);
    let verified = models.verify(MODEL).await.unwrap();
    assert!(verified.ok, "checksum after download");
    // One flipped byte ⇒ corrupted; restored ⇒ intact again.
    let path = std::path::PathBuf::from(&installed.path);
    let flip = |p: &std::path::Path| {
        use std::io::{Read, Seek, SeekFrom, Write};
        let mut f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(p)
            .unwrap();
        f.seek(SeekFrom::Start(1_000_000)).unwrap();
        let mut b = [0u8; 1];
        f.read_exact(&mut b).unwrap();
        f.seek(SeekFrom::Start(1_000_000)).unwrap();
        f.write_all(&[b[0] ^ 0xFF]).unwrap();
    };
    flip(&path);
    assert!(
        !models.verify(MODEL).await.unwrap().ok,
        "a corrupted model is detected"
    );
    flip(&path);
    assert!(models.verify(MODEL).await.unwrap().ok);
    models.activate(MODEL).await.unwrap();
    report["model"] = json!({
        "id": MODEL,
        "bytes": installed.size,
        "sha256": installed.sha256,
        "download_secs": download_secs,
        "progress_events": *progress_events.lock().unwrap(),
        "checksum_ok": true,
        "corruption_detected": true,
    });

    // ── 2. From here on: offline (only loopback / Unix sockets) ──
    // The Model Manager's HTTP client keeps the download's connections pooled: drop it first.
    drop(models);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let watch = NetworkWatch::start();
    let config =
        nlmx_embed_llama::EmbeddingConfig::load(&nlmx_embed_llama::config_path(&data)).unwrap();
    let llama_bin = nlmx_embed_llama::llama_server_path().unwrap();
    assert!(
        llama_bin.starts_with(&app_path),
        "llama-server from the app: {}",
        llama_bin.display()
    );
    let llama = Arc::new(nlmx_embed_llama::provider(config, llama_bin, &data));
    let identity = llama.identity().await.unwrap();
    assert_eq!(identity.dimensions, 1024);
    let fm = Arc::new(nlmx_llm_fm::FoundationModelsProvider::system(
        data.join("run"),
    ));
    assert!(
        fm.status().await.is_available(),
        "Apple Foundation Models available"
    );

    // ── 3. Ingestion with the bundled PDFium + llama.cpp ──
    let mut library = Library::new(data.clone(), FixedEmbeddingSource::of_arc(llama.clone()));
    let corpus = [
        "report.pdf",
        "text.pdf",
        "unicode.pdf",
        "large.pdf",
        "rotated.pdf",
    ];
    let started = Instant::now();
    library.import(&corpus).await;
    let encrypted = library.ingestion.import(&fixture("encrypted.pdf")).await;
    assert!(
        matches!(encrypted, ImportOutcome::Failed { .. }),
        "password-protected PDF is refused cleanly"
    );
    report["ingestion_secs"] = json!(started.elapsed().as_secs_f64());

    // ── 4. RAG (golden set) with Apple Foundation Models ──
    let golden: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("tests/golden/rag.json")).unwrap(),
    )
    .unwrap();
    let rag = library.rag(fm.clone());
    let (mut answerable, mut cited_right, mut not_found_right, mut unanswerable) = (0, 0, 0, 0);
    for q in golden["questions"].as_array().unwrap() {
        let question = q["question"].as_str().unwrap();
        let expect: Vec<(String, u32)> = q["expect"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e[0].as_str().unwrap().to_string(),
                    e[1].as_u64().unwrap() as u32,
                )
            })
            .collect();
        let answer = rag
            .ask(
                question,
                &RagOptions::default(),
                &|_| {},
                CancelFlag::default(),
            )
            .await
            .unwrap();
        if expect.is_empty() {
            unanswerable += 1;
            not_found_right += usize::from(answer.status == AnswerStatus::NotFound);
        } else {
            answerable += 1;
            let ok = answer.citations.iter().any(|c| {
                expect.iter().any(|(f, p)| {
                    library.file_of(c.document_id) == f && (c.page_start..=c.page_end).contains(p)
                })
            });
            cited_right += usize::from(ok);
            if !ok {
                eprintln!(
                    "✗ citação: {question} → {:?} {:?}",
                    answer.status, answer.answer
                );
            }
        }
    }
    report["rag"] = json!({
        "answerable": answerable,
        "citations_valid": cited_right,
        "unanswerable": unanswerable,
        "not_found_correct": not_found_right,
    });
    assert!(cited_right as f64 / answerable as f64 >= 0.8, "{report}");
    assert_eq!(not_found_right, unanswerable);

    // ── 5. Chat (conversation, intents, follow-up) and the viewer, through the UI router ──
    let chat = Arc::new(library.chat(fm.clone(), RagOptions::default()));
    let report_id = library.documents["report.pdf"];
    let conversation = chat.start(Some(report_id)).await.unwrap();
    let mut chat_results = Vec::new();
    for question in [
        "Explique este documento.",
        "Quais são os principais pontos da seção 3?",
        "Quando termina o prazo de carência?",
        "E a partir de quando os prazos contam?",
    ] {
        let (_, id) = chat.ask(conversation.id, question).await.unwrap();
        let tokens = Arc::new(Mutex::new(0usize));
        let t = tokens.clone();
        let m = chat
            .answer(id, &move |_| *t.lock().unwrap() += 1, CancelFlag::default())
            .await
            .unwrap();
        assert_eq!(
            m.status,
            MessageStatus::Answered,
            "{question}: {}",
            m.content
        );
        assert!(
            m.sources.iter().any(|s| s.cited),
            "{question}: cites a source"
        );
        assert!(*tokens.lock().unwrap() > 1, "streamed");
        chat_results.push(
            json!({ "question": question, "cited": m.sources.iter().filter(|s| s.cited).count() }),
        );
    }
    report["chat"] = json!(chat_results);

    let app = nlmx_ui_web::router(nlmx_ui_web::AppState {
        system_status: Arc::new(GetSystemStatus::new(
            fm.clone(),
            library.db.clone(),
            Ok(nlmx_pdf_pdfium::PDFIUM_BUILD.into()),
            Arc::new(nlmx_embed_llama::LlamaCppRuntime::detect()),
        )),
        ingestion: Err("not used".into()),
        chat: Ok(chat.clone()),
        viewer: Ok(Arc::new(library.viewer())),
        remover: Err("remoção indisponível neste teste".into()),
        diagnostics: None,
        models: None,
    });
    let last = chat
        .messages(conversation.id)
        .await
        .unwrap()
        .last()
        .unwrap()
        .id;
    let (_, answer_html) = get(&app, &format!("/chat/messages/{last}")).await;
    let citation = attr(&answer_html, r#"class="citation""#, "hx-get").replace("&amp;", "&");
    let (status, viewer) = get(&app, &citation).await;
    assert_eq!(status, 200);
    assert!(viewer.contains("viewer-highlight"), "highlighted passage");
    let page = attr(&viewer, "data-viewer ", "data-target-page").to_string();
    let (_, search) = get(&app, &format!("/viewer/{report_id}/search?q=car%C3%AAncia")).await;
    let hits = serde_json::from_str::<Value>(&search).unwrap()["hits"]
        .as_array()
        .unwrap()
        .len();
    let (png_status, _) = get(
        &app,
        &format!("/documents/{report_id}/pages/{page}.png?w=900"),
    )
    .await;
    let (text_status, text) = get(&app, &format!("/viewer/{report_id}/pages/{page}/text")).await;
    assert!(hits > 0 && png_status == 200 && text_status == 200 && text.contains("<span"));
    report["viewer"] = json!({ "citation": citation, "page": page, "search_hits": hits });

    // ── 6. Measurements, network, cleanup ──
    let layout = DataLayout {
        database: data.join("nlmx.sqlite3"),
        library: data.join("library"),
        models: data.join("models"),
        logs: data.join("logs"),
        run: data.join("run"),
    };
    let resources = layout.sample_resources();
    metrics.record(resources.name(), &resources.to_json());
    let storage = layout.sample_storage();
    metrics.record(storage.name(), &storage.to_json());
    fm.shutdown().await;
    llama.shutdown().await;
    let (remotes, external) = watch.finish();
    report["network"] = json!({ "remotes_seen": remotes, "external": external });
    let s = metrics.snapshot();
    report["metrics"] = snapshot_json(&s);
    assert_eq!(s.operation(Operation::Generate).unwrap().failed, 0);
    let out = root().join("target/acceptance");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(
        out.join("report.json"),
        serde_json::to_string_pretty(&report).unwrap(),
    )
    .unwrap();
    println!("ACCEPTANCE {}", serde_json::to_string(&report).unwrap());
    assert!(
        external.is_empty(),
        "offline run opened external connections: {external:?}"
    );
}
