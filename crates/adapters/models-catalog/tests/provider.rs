//! `LocalModelProvider` against a local HTTP server (with Range support and failure modes).

use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use nlmx_application::ports::{CancelFlag, ModelProvider, ProgressCallback};
use nlmx_domain::models::{DownloadProgress, License, ModelDescriptor, ModelError, ModelState};
use nlmx_models_catalog::{LocalModelProvider, builtin_catalog, parse_catalog};
use sha2::{Digest, Sha256};

const MB: usize = 1024 * 1024;

#[derive(Clone, Default)]
struct Mode {
    /// Close the connection after this many body bytes, on the first request only.
    drop_after: Option<usize>,
    /// Flip a byte in the middle of the body.
    corrupt: bool,
    /// Pause between 64 KiB writes.
    delay: Option<Duration>,
}

struct Server {
    url: String,
    /// Range header of every request (None = full download).
    ranges: Arc<Mutex<Vec<Option<String>>>>,
}

fn serve(content: Arc<Vec<u8>>, mode: Mode) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/model.gguf", listener.local_addr().unwrap());
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&ranges);
    let first = Arc::new(AtomicUsize::new(0));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (content, mode, seen, first) =
                (content.clone(), mode.clone(), seen.clone(), first.clone());
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut range = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range:") {
                        range = Some(v.trim().to_string());
                    }
                }
                seen.lock().unwrap().push(range.clone());
                let start = range
                    .as_deref()
                    .and_then(|r| r.strip_prefix("bytes="))
                    .and_then(|r| r.trim_end_matches('-').parse::<usize>().ok())
                    .unwrap_or(0);
                let mut body = content[start..].to_vec();
                if mode.corrupt {
                    let mid = body.len() / 2;
                    body[mid] ^= 0xFF;
                }
                let head = if start > 0 {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{}/{}\r\n\r\n",
                        body.len(),
                        content.len() - 1,
                        content.len()
                    )
                } else {
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len())
                };
                let _ = stream.write_all(head.as_bytes());
                let limit = match (mode.drop_after, first.fetch_add(1, Ordering::SeqCst)) {
                    (Some(n), 0) => n.min(body.len()),
                    _ => body.len(),
                };
                for piece in body[..limit].chunks(64 * 1024) {
                    if stream.write_all(piece).is_err() {
                        return;
                    }
                    if let Some(d) = mode.delay {
                        std::thread::sleep(d);
                    }
                }
                let _ = stream.flush();
            });
        }
    });
    Server { url, ranges }
}

/// A fake GGUF: magic header + deterministic bytes.
fn content(size: usize) -> Arc<Vec<u8>> {
    let mut bytes = b"GGUF".to_vec();
    let mut x: u32 = 1;
    while bytes.len() < size {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        bytes.push((x >> 16) as u8);
    }
    Arc::new(bytes)
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn descriptor(url: &str, bytes: &[u8], version: &str) -> ModelDescriptor {
    ModelDescriptor {
        id: "test-model".into(),
        display_name: "Test".into(),
        description: "Test model".into(),
        version: version.into(),
        url: url.into(),
        file_name: "test.gguf".into(),
        size: bytes.len() as u64,
        sha256: sha(bytes),
        license: License {
            id: "MIT".into(),
            url: "https://example.com".into(),
        },
        languages: vec!["pt".into()],
        dimensions: 8,
        pooling: "mean".into(),
        query_prefix: "query: ".into(),
        passage_prefix: "passage: ".into(),
        context_size: 512,
        recommended: true,
    }
}

fn data_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nlmx-models-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn provider(dir: &Path, catalog: Vec<ModelDescriptor>) -> LocalModelProvider {
    LocalModelProvider::new(dir.join("models"), dir.join("embedding.json"), catalog)
        .with_disk_meter(Arc::new(|_: &Path| Ok(100 * 1024 * MB as u64)))
}

fn recorder() -> (ProgressCallback, Arc<Mutex<Vec<DownloadProgress>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    (Arc::new(move |p| sink.lock().unwrap().push(p)), events)
}

async fn install(
    p: &LocalModelProvider,
) -> Result<nlmx_domain::models::InstalledModel, ModelError> {
    let plan = p.plan_download("test-model").await?;
    let (progress, _) = recorder();
    p.download(plan.confirm(), progress, CancelFlag::default())
        .await
}

#[test]
fn the_embedded_catalog_is_valid() {
    let catalog = builtin_catalog();
    assert!(catalog.iter().any(|m| m.recommended));
    for m in &catalog {
        assert!(
            m.url.contains(&m.version),
            "{}: URL pinned to the version",
            m.id
        );
        assert!(m.url.starts_with("https://huggingface.co/"), "{}", m.id);
        assert_eq!(m.sha256.len(), 64);
        assert!(m.dimensions > 0 && m.size > 100 * MB as u64);
    }
    assert!(
        parse_catalog(r#"[{"id": "x"}]"#).is_err(),
        "incomplete entries are rejected"
    );
}

#[tokio::test]
async fn downloads_with_progress_checksum_and_manifest() {
    let bytes = content(3 * MB);
    let server = serve(bytes.clone(), Mode::default());
    let dir = data_dir("download");
    let p = provider(&dir, vec![descriptor(&server.url, &bytes, "v1")]);
    assert_eq!(
        p.status("test-model").await.unwrap(),
        ModelState::NotInstalled
    );
    assert!(p.installed().await.unwrap().is_empty());

    let plan = p.plan_download("test-model").await.unwrap();
    assert_eq!((plan.resume_from, plan.replaces.clone()), (0, None));
    assert_eq!(plan.model.license.id, "MIT");
    assert!(
        plan.required_bytes > plan.model.size,
        "includes a safety margin"
    );
    assert!(plan.fits_on_disk());

    let (progress, events) = recorder();
    let installed = p
        .download(plan.confirm(), progress, CancelFlag::default())
        .await
        .unwrap();
    assert_eq!(installed.version, "v1");
    assert_eq!(installed.sha256, sha(&bytes));
    assert!(
        installed.path.ends_with("models/test-model/v1/test.gguf"),
        "{}",
        installed.path
    );
    assert_eq!(std::fs::read(&installed.path).unwrap(), *bytes);
    assert!(
        Path::new(&installed.path)
            .with_file_name("manifest.json")
            .exists()
    );

    let events = events.lock().unwrap().clone();
    assert!(
        events.windows(2).all(|w| w[0].received <= w[1].received),
        "progress only grows"
    );
    let last = events.last().unwrap();
    assert_eq!(
        (last.received, last.total),
        (bytes.len() as u64, bytes.len() as u64)
    );
    assert!((last.fraction() - 1.0).abs() < f64::EPSILON);

    assert_eq!(
        p.status("test-model").await.unwrap(),
        ModelState::Installed {
            version: "v1".into()
        }
    );
    assert_eq!(p.installed().await.unwrap().len(), 1);
    assert!(matches!(
        p.plan_download("test-model").await,
        Err(ModelError::AlreadyInstalled(_))
    ));
    assert!(p.disk().await.unwrap().models_bytes >= bytes.len() as u64);
}

#[tokio::test]
async fn a_checksum_mismatch_installs_nothing() {
    let bytes = content(2 * MB);
    let server = serve(
        bytes.clone(),
        Mode {
            corrupt: true,
            ..Mode::default()
        },
    );
    let dir = data_dir("mismatch");
    let p = provider(&dir, vec![descriptor(&server.url, &bytes, "v1")]);
    let err = install(&p).await.unwrap_err();
    assert!(
        matches!(err, ModelError::ChecksumMismatch { .. }),
        "{err:?}"
    );
    assert_eq!(
        p.status("test-model").await.unwrap(),
        ModelState::NotInstalled,
        "partial file discarded"
    );
}

#[tokio::test]
async fn an_interrupted_download_resumes_with_range() {
    let bytes = content(3 * MB);
    let server = serve(
        bytes.clone(),
        Mode {
            drop_after: Some(MB + 12_345),
            ..Mode::default()
        },
    );
    let dir = data_dir("resume");
    let p = provider(&dir, vec![descriptor(&server.url, &bytes, "v1")]);

    let err = install(&p).await.unwrap_err();
    assert!(matches!(err, ModelError::Network(_)), "{err:?}");
    let ModelState::PartiallyDownloaded {
        bytes: partial,
        total,
    } = p.status("test-model").await.unwrap()
    else {
        panic!("partial download kept")
    };
    assert_eq!((partial, total), ((MB + 12_345) as u64, bytes.len() as u64));
    assert_eq!(
        p.plan_download("test-model").await.unwrap().resume_from,
        partial
    );

    let installed = install(&p).await.unwrap();
    assert_eq!(
        std::fs::read(&installed.path).unwrap(),
        *bytes,
        "resumed file is byte-identical"
    );
    let ranges = server.ranges.lock().unwrap().clone();
    assert_eq!(ranges, [None, Some(format!("bytes={partial}-"))]);
}

#[tokio::test]
async fn cancelling_keeps_the_partial_download() {
    let bytes = content(4 * MB);
    let server = serve(
        bytes.clone(),
        Mode {
            delay: Some(Duration::from_millis(20)),
            ..Mode::default()
        },
    );
    let dir = data_dir("cancel");
    let p = provider(&dir, vec![descriptor(&server.url, &bytes, "v1")]);
    let plan = p.plan_download("test-model").await.unwrap();
    let cancel = CancelFlag::default();
    let trigger = cancel.clone();
    let progress: ProgressCallback = Arc::new(move |e: DownloadProgress| {
        if e.received > MB as u64 {
            trigger.cancel();
        }
    });
    assert_eq!(
        p.download(plan.confirm(), progress, cancel)
            .await
            .unwrap_err(),
        ModelError::Cancelled
    );
    assert!(matches!(
        p.status("test-model").await.unwrap(),
        ModelState::PartiallyDownloaded { .. }
    ));
}

#[tokio::test]
async fn refuses_to_start_without_enough_disk_space() {
    let bytes = content(2 * MB);
    let server = serve(bytes.clone(), Mode::default());
    let dir = data_dir("disk");
    let p = LocalModelProvider::new(
        dir.join("models"),
        dir.join("embedding.json"),
        vec![descriptor(&server.url, &bytes, "v1")],
    )
    .with_disk_meter(Arc::new(|_: &Path| Ok(MB as u64)));
    let plan = p.plan_download("test-model").await.unwrap();
    assert!(!plan.fits_on_disk());
    let (progress, _) = recorder();
    let err = p
        .download(plan.confirm(), progress, CancelFlag::default())
        .await
        .unwrap_err();
    assert!(
        matches!(err, ModelError::InsufficientSpace { .. }),
        "{err:?}"
    );
    assert!(
        server.ranges.lock().unwrap().is_empty(),
        "no request was made"
    );
}

#[tokio::test]
async fn detects_corrupted_models() {
    let bytes = content(2 * MB);
    let server = serve(bytes.clone(), Mode::default());
    let dir = data_dir("corrupt");
    let p = provider(&dir, vec![descriptor(&server.url, &bytes, "v1")]);
    let installed = install(&p).await.unwrap();
    assert!(p.verify("test-model").await.unwrap().ok);

    // A flipped byte keeps size and magic: only the full checksum notices.
    let mut data = std::fs::read(&installed.path).unwrap();
    data[MB] ^= 0x01;
    std::fs::write(&installed.path, &data).unwrap();
    assert_eq!(
        p.status("test-model").await.unwrap(),
        ModelState::Installed {
            version: "v1".into()
        }
    );
    let report = p.verify("test-model").await.unwrap();
    assert!(!report.ok && report.actual_sha256 != installed.sha256);

    // Truncation and a wrong header are caught by the quick check.
    std::fs::write(&installed.path, &data[..MB]).unwrap();
    assert!(matches!(
        p.status("test-model").await.unwrap(),
        ModelState::Corrupted { .. }
    ));
    assert!(p.installed().await.unwrap().is_empty());
    data[0] = b'X';
    std::fs::write(&installed.path, &data).unwrap();
    let ModelState::Corrupted { reason } = p.status("test-model").await.unwrap() else {
        panic!()
    };
    assert!(reason.contains("GGUF"), "{reason}");
}

#[tokio::test]
async fn activation_writes_the_embedding_config_and_removal_clears_it() {
    let bytes = content(MB);
    let server = serve(bytes.clone(), Mode::default());
    let dir = data_dir("activate");
    let p = provider(&dir, vec![descriptor(&server.url, &bytes, "v1")]);
    assert!(matches!(
        p.activate("test-model").await,
        Err(ModelError::NotInstalled(_))
    ));
    let installed = install(&p).await.unwrap();
    assert_eq!(
        p.active().await.unwrap(),
        None,
        "installing does not activate"
    );

    p.activate("test-model").await.unwrap();
    assert_eq!(p.active().await.unwrap(), Some(installed.clone()));
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("embedding.json")).unwrap())
            .unwrap();
    assert_eq!(config["model_id"], "test-model");
    assert_eq!(config["model_path"], installed.path);
    assert_eq!(config["query_prefix"], "query: ");

    p.remove("test-model").await.unwrap();
    assert!(
        !dir.join("embedding.json").exists(),
        "active model removed ⇒ config removed"
    );
    assert!(!dir.join("models/test-model").exists());
    assert_eq!(
        p.status("test-model").await.unwrap(),
        ModelState::NotInstalled
    );
    p.remove("test-model").await.unwrap(); // idempotent
    assert!(matches!(
        p.status("other").await,
        Err(ModelError::UnknownModel(_))
    ));
}

#[tokio::test]
async fn updates_to_a_new_version_and_drops_the_old_one() {
    let v1 = content(MB);
    let mut v2 = (*content(MB + 777)).clone();
    v2[100] = 42;
    let v2 = Arc::new(v2);
    let s1 = serve(v1.clone(), Mode::default());
    let s2 = serve(v2.clone(), Mode::default());
    let dir = data_dir("update");

    let old = provider(&dir, vec![descriptor(&s1.url, &v1, "v1")]);
    install(&old).await.unwrap();
    old.activate("test-model").await.unwrap();
    assert!(matches!(
        old.plan_update("test-model").await,
        Err(ModelError::AlreadyInstalled(_))
    ));

    // A newer catalog (app update) ships v2.
    let new = provider(&dir, vec![descriptor(&s2.url, &v2, "v2")]);
    assert_eq!(
        new.status("test-model").await.unwrap(),
        ModelState::UpdateAvailable {
            installed: "v1".into(),
            latest: "v2".into()
        }
    );
    let plan = new.plan_update("test-model").await.unwrap();
    assert_eq!(plan.replaces.as_deref(), Some("v1"));
    let (progress, _) = recorder();
    let installed = new
        .download(plan.confirm(), progress, CancelFlag::default())
        .await
        .unwrap();
    assert_eq!(installed.version, "v2");
    assert_eq!(
        new.status("test-model").await.unwrap(),
        ModelState::Installed {
            version: "v2".into()
        }
    );
    assert!(
        !dir.join("models/test-model/v1").exists(),
        "old version removed"
    );
    assert_eq!(
        new.active().await.unwrap().map(|m| m.version),
        Some("v2".into()),
        "active model follows the update"
    );
}

#[tokio::test]
async fn one_download_per_model_at_a_time() {
    let bytes = content(2 * MB);
    let server = serve(
        bytes.clone(),
        Mode {
            delay: Some(Duration::from_millis(10)),
            ..Mode::default()
        },
    );
    let dir = data_dir("concurrent");
    let p = Arc::new(provider(&dir, vec![descriptor(&server.url, &bytes, "v1")]));
    let plan = p.plan_download("test-model").await.unwrap();
    let (a, b) = (plan.clone().confirm(), plan.confirm());
    let first = {
        let p = Arc::clone(&p);
        tokio::spawn(async move {
            let (progress, _) = recorder();
            p.download(a, progress, CancelFlag::default()).await
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (progress, _) = recorder();
    let second = p.download(b, progress, CancelFlag::default()).await;
    assert!(
        matches!(second, Err(ModelError::Io(ref m)) if m.contains("já está sendo baixado")),
        "{second:?}"
    );
    assert!(first.await.unwrap().is_ok());
}
