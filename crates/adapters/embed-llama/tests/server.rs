//! `LlamaServer` lifecycle against the fake server binary.

mod common;

use std::{path::PathBuf, time::Duration};

use common::{alive, data_dir, fake_config, wait_dead};
use nlmx_domain::embedding::EmbeddingError;
use nlmx_embed_llama::{Health, LlamaServer};

fn pidfile(dir: &std::path::Path) -> PathBuf {
    dir.join("run/llama-server.json")
}

#[tokio::test]
async fn start_health_stop() {
    let dir = data_dir("lifecycle");
    let server = LlamaServer::new(fake_config(&dir, &[("FAKE_LOAD_MS", "400")]));
    assert!(
        matches!(server.health().await, Health::Down(_)),
        "not started yet"
    );

    let endpoint = server.start().await.unwrap();
    assert!(
        endpoint.base_url.starts_with("http://127.0.0.1:"),
        "{}",
        endpoint.base_url
    );
    assert_eq!(server.health().await, Health::Ready);
    let pid = server.pid().await.unwrap();
    assert!(alive(pid));
    assert!(pidfile(&dir).exists());

    // Starting again while healthy is a no-op.
    assert_eq!(server.start().await.unwrap(), endpoint);
    assert_eq!(server.pid().await, Some(pid));

    server.stop().await;
    assert!(wait_dead(pid).await, "process stopped");
    assert!(!pidfile(&dir).exists(), "pidfile removed");
    assert!(matches!(server.health().await, Health::Down(_)));
    server.stop().await; // idempotent
}

#[tokio::test]
async fn listens_only_on_loopback_and_requires_the_api_key() {
    let dir = data_dir("security");
    let server = LlamaServer::new(fake_config(&dir, &[]));
    let endpoint = server.start().await.unwrap();
    let pid = server.pid().await.unwrap();

    let output = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", "-a", "-p", &pid.to_string(), "-iTCP", "-sTCP:LISTEN"])
        .output()
        .unwrap();
    let listening = String::from_utf8_lossy(&output.stdout);
    assert!(listening.contains("127.0.0.1:"), "{listening}");
    assert!(
        !listening.contains("*:"),
        "must not listen on all interfaces: {listening}"
    );

    let http = reqwest::Client::new();
    let url = format!("{}/v1/models", endpoint.base_url);
    assert_eq!(http.get(&url).send().await.unwrap().status(), 401);
    assert_eq!(
        http.get(&url)
            .bearer_auth(&endpoint.api_key)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    // The key is never on the command line, and its file is private.
    let command = std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&command.stdout).contains(&endpoint.api_key));
    let mode = std::os::unix::fs::PermissionsExt::mode(
        &std::fs::metadata(dir.join("run/llama-server.key"))
            .unwrap()
            .permissions(),
    );
    assert_eq!(mode & 0o777, 0o600);
    server.stop().await;
}

#[tokio::test]
async fn restart_replaces_the_process() {
    let dir = data_dir("restart");
    let server = LlamaServer::new(fake_config(&dir, &[]));
    server.start().await.unwrap();
    let first = server.pid().await.unwrap();
    server.restart().await.unwrap();
    let second = server.pid().await.unwrap();
    assert_ne!(first, second);
    assert!(wait_dead(first).await);
    assert_eq!(server.health().await, Health::Ready);
    server.stop().await;
}

#[tokio::test]
async fn startup_timeout_kills_the_process_and_reports_logs() {
    let dir = data_dir("timeout");
    let mut config = fake_config(&dir, &[("FAKE_NEVER_READY", "1")]);
    config.startup_timeout = Duration::from_millis(800);
    let server = LlamaServer::new(config);
    let err = server.start().await.unwrap_err();
    let EmbeddingError::Timeout(message) = err else {
        panic!("{err:?}")
    };
    assert!(
        message.contains("fake: server is listening"),
        "includes recent logs: {message}"
    );
    assert_eq!(server.pid().await, None);
    assert!(!pidfile(&dir).exists());
    // No fake server left running from this data dir.
    let leftovers = std::process::Command::new("/usr/bin/pgrep")
        .args(["-f", &dir.display().to_string()])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&leftovers.stdout).trim().is_empty(),
        "process killed after timeout"
    );
}

#[tokio::test]
async fn a_server_that_dies_during_startup_reports_why() {
    let dir = data_dir("crash-start");
    let server = LlamaServer::new(fake_config(&dir, &[("FAKE_EXIT_AT_START", "1")]));
    let err = server.start().await.unwrap_err();
    let EmbeddingError::Unavailable(message) = err else {
        panic!("{err:?}")
    };
    assert!(message.contains("failed to load model"), "{message}");
}

#[tokio::test]
async fn missing_binary_or_model_is_unavailable() {
    let dir = data_dir("missing");
    let mut config = fake_config(&dir, &[]);
    config.binary = PathBuf::from("/nonexistent/llama-server");
    assert!(matches!(
        LlamaServer::new(config).start().await,
        Err(EmbeddingError::Unavailable(_))
    ));

    let mut config = fake_config(&dir, &[]);
    config.model_path = dir.join("missing.gguf");
    assert!(matches!(
        LlamaServer::new(config).start().await,
        Err(EmbeddingError::Unavailable(_))
    ));
}

#[tokio::test]
async fn server_output_goes_to_the_log_file_and_recent_logs() {
    let dir = data_dir("logs");
    let server = LlamaServer::new(fake_config(&dir, &[]));
    server.start().await.unwrap();
    assert!(
        server
            .recent_logs()
            .iter()
            .any(|l| l.contains("fake: loading model"))
    );
    let file = std::fs::read_to_string(dir.join("logs/llama-server.log")).unwrap();
    assert!(
        file.contains("--- starting") && file.contains("fake: server is listening"),
        "{file}"
    );
    server.stop().await;
}

#[tokio::test]
async fn reuses_a_healthy_server_from_a_previous_session() {
    let dir = data_dir("adopt");
    let first = LlamaServer::new(fake_config(&dir, &[]));
    let endpoint = first.start().await.unwrap();
    let pid = first.pid().await.unwrap();
    // Simulate the app quitting without stopping it (the supervisor is forgotten, not dropped).
    std::mem::forget(first);

    let second = LlamaServer::new(fake_config(&dir, &[]));
    assert_eq!(
        second.start().await.unwrap(),
        endpoint,
        "same server reused"
    );
    assert_eq!(second.pid().await, Some(pid));
    assert_eq!(second.health().await, Health::Ready);
    second.stop().await;
    assert!(wait_dead(pid).await, "an adopted server can be stopped too");
}

#[tokio::test]
async fn replaces_a_previous_server_running_another_model() {
    let dir = data_dir("replace");
    let first = LlamaServer::new(fake_config(&dir, &[]));
    first.start().await.unwrap();
    let old = first.pid().await.unwrap();
    std::mem::forget(first);

    let mut config = fake_config(&dir, &[]);
    config.model_path = dir.join("other.gguf");
    std::fs::write(&config.model_path, b"GGUF other").unwrap();
    let second = LlamaServer::new(config);
    second.start().await.unwrap();
    assert_ne!(second.pid().await, Some(old));
    assert!(wait_dead(old).await, "old server stopped");
    second.stop().await;
}

#[tokio::test]
async fn a_pidfile_pointing_at_someone_elses_process_is_ignored() {
    let dir = data_dir("foreign");
    // A live process that is not llama-server.
    let mut sleeper = std::process::Command::new("/bin/sleep")
        .arg("30")
        .spawn()
        .unwrap();
    std::fs::create_dir_all(dir.join("run")).unwrap();
    std::fs::write(
        pidfile(&dir),
        format!(
            r#"{{"pid":{},"port":1,"binary":"{}","model_path":"/x.gguf"}}"#,
            sleeper.id(),
            common::FAKE
        ),
    )
    .unwrap();

    let server = LlamaServer::new(fake_config(&dir, &[]));
    server.start().await.unwrap();
    assert!(alive(sleeper.id()), "foreign process untouched");
    assert_ne!(server.pid().await, Some(sleeper.id()));
    server.stop().await;
    sleeper.kill().unwrap();
    sleeper.wait().unwrap();
}

#[tokio::test]
async fn health_notices_a_crashed_process() {
    let dir = data_dir("health-crash");
    let server = LlamaServer::new(fake_config(&dir, &[]));
    server.start().await.unwrap();
    let pid = server.pid().await.unwrap();
    std::process::Command::new("/bin/kill")
        .args(["-KILL", &pid.to_string()])
        .status()
        .unwrap();
    assert!(wait_dead(pid).await);
    assert!(matches!(server.health().await, Health::Down(_)));
    // start() brings it back.
    server.start().await.unwrap();
    assert_eq!(server.health().await, Health::Ready);
    server.stop().await;
}

#[tokio::test]
async fn disables_the_prompt_cache_and_uses_one_slot() {
    let dir = data_dir("args");
    let args_file = dir.join("args.txt");
    let server = LlamaServer::new(fake_config(
        &dir,
        &[("FAKE_ARGS_OUT", args_file.to_str().unwrap())],
    ));
    server.start().await.unwrap();
    let args = std::fs::read_to_string(&args_file).unwrap();
    let args: Vec<&str> = args.lines().collect();
    for pair in [["--cache-ram", "0"], ["--parallel", "1"]] {
        assert!(args.windows(2).any(|w| w == pair), "{pair:?} in {args:?}");
    }
    assert!(args.contains(&"--no-cache-prompt"), "{args:?}");
    server.stop().await;
}
