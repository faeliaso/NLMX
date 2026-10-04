//! `FoundationModelsProvider` against `fake-fm`: supervision, streaming, refusals, cancellation, counts.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use nlmx_application::ports::{CancelFlag, LlmProvider};
use nlmx_domain::generation::{
    FinishReason, GenerationRequest, LanguageModelStatus, LlmError, UnavailableKind,
};
use nlmx_llm_fm::{FoundationModelsConfig, FoundationModelsProvider, MAX_SOCKET_PATH};

const FAKE: &str = env!("CARGO_BIN_EXE_fake-fm");

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nlmx-fm-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn provider(name: &str, env: &[(&str, &str)]) -> (FoundationModelsProvider, PathBuf) {
    let dir = dir(name);
    let mut config = FoundationModelsConfig::new(FAKE, dir.join("run"));
    config.extra_env = env
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    config.startup_timeout = Duration::from_secs(5);
    config.idle_timeout = Duration::from_secs(5);
    (FoundationModelsProvider::new(config), dir)
}

fn request() -> GenerationRequest {
    GenerationRequest {
        system: "Responda com base nos trechos.".into(),
        history: Vec::new(),
        user: "<pergunta>\nQual a carência?\n</pergunta>".into(),
        temperature: 0.2,
        max_tokens: 100,
    }
}

fn collector() -> (Arc<Mutex<Vec<String>>>, impl Fn(&str) + Send + Sync) {
    let tokens = Arc::new(Mutex::new(Vec::new()));
    let sink = tokens.clone();
    (tokens, move |t: &str| {
        sink.lock().unwrap().push(t.to_string())
    })
}

#[tokio::test]
async fn streams_the_answer_and_sends_system_and_user_messages() {
    let record = dir("record").join("requests.jsonl");
    let record_s = record.display().to_string();
    let (fm, _) = provider(
        "stream",
        &[
            ("FAKE_FM_ANSWER", "A carência é de 180 dias [1]."),
            ("FAKE_FM_RECORD", &record_s),
        ],
    );
    let (tokens, on_token) = collector();
    let generation = fm
        .generate(&request(), &on_token, CancelFlag::default())
        .await
        .unwrap();
    assert_eq!(generation.text, "A carência é de 180 dias [1].");
    assert_eq!(generation.finish, FinishReason::Completed);
    assert!(tokens.lock().unwrap().len() > 1, "streamed in pieces");

    let body: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(&record)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["model"], "system");
    assert_eq!(body["stream"], true);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(
        body["messages"][0]["content"],
        "Responda com base nos trechos."
    );
    assert_eq!(body["messages"][1]["role"], "user");

    // The server is reused for the next generation.
    fm.generate(&request(), &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&record).unwrap().lines().count(), 2);
    fm.shutdown().await;
}

/// Earlier turns go to `fm serve` as conversation turns; `fm respond` gets them in the prompt.
#[tokio::test]
async fn earlier_turns_are_sent_as_conversation_turns() {
    let mut with_history = request();
    with_history.history = vec![nlmx_domain::generation::ChatTurn {
        user: "meu nome é Ana".into(),
        assistant: "Olá, Ana!".into(),
    }];
    with_history.user = "qual é o meu nome?".into();

    let record = dir("turns-record").join("requests.jsonl");
    let record_s = record.display().to_string();
    let (fm, _) = provider("turns", &[("FAKE_FM_RECORD", &record_s)]);
    fm.generate(&with_history, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    fm.shutdown().await;
    let body: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(&record)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let messages: Vec<(&str, &str)> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["role"].as_str().unwrap(), m["content"].as_str().unwrap()))
        .collect();
    assert_eq!(
        messages,
        [
            ("system", "Responda com base nos trechos."),
            ("user", "meu nome é Ana"),
            ("assistant", "Olá, Ana!"),
            ("user", "qual é o meu nome?"),
        ]
    );

    let record = dir("turns-fallback-record").join("requests.jsonl");
    let record_s = record.display().to_string();
    let (fm, _) = provider(
        "turns-fallback",
        &[("FAKE_FM_SERVE_EXIT", "1"), ("FAKE_FM_RECORD", &record_s)],
    );
    fm.generate(&with_history, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    let line: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(&record)
            .unwrap()
            .lines()
            .last()
            .unwrap(),
    )
    .unwrap();
    let args: Vec<&str> = line["respond"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    let i = args.iter().position(|a| *a == "-i").unwrap();
    assert_eq!(args[i + 2], with_history.flat_user());
}

#[tokio::test]
async fn guardrail_errors_are_refusals() {
    let (fm, _) = provider("refuse", &[("FAKE_FM_MODE", "refuse")]);
    let err = fm
        .generate(&request(), &|_| {}, CancelFlag::default())
        .await
        .unwrap_err();
    assert!(matches!(err, LlmError::Refused(m) if m.contains("guardrails")));
    fm.shutdown().await;
}

#[tokio::test]
async fn length_finish_is_reported() {
    let (fm, _) = provider("length", &[("FAKE_FM_MODE", "length")]);
    let g = fm
        .generate(&request(), &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    assert_eq!(g.finish, FinishReason::Length);
    fm.shutdown().await;
}

#[tokio::test]
async fn cancelling_returns_the_partial_answer() {
    let (fm, _) = provider(
        "cancel",
        &[
            ("FAKE_FM_ANSWER", "um dois três quatro cinco seis sete oito"),
            ("FAKE_FM_DELAY_MS", "150"),
        ],
    );
    let cancel = CancelFlag::default();
    let trigger = cancel.clone();
    let on_token = move |t: &str| {
        if t.starts_with("dois") {
            trigger.cancel();
        }
    };
    let g = fm.generate(&request(), &on_token, cancel).await.unwrap();
    assert_eq!(g.finish, FinishReason::Cancelled);
    assert!(g.text.starts_with("um dois"), "{:?}", g.text);
    assert!(!g.text.contains("oito"));
    fm.shutdown().await;
}

#[tokio::test]
async fn falls_back_to_fm_respond_when_serve_does_not_start() {
    let record = dir("fallback-record").join("requests.jsonl");
    let record_s = record.display().to_string();
    let (fm, _) = provider(
        "fallback",
        &[
            ("FAKE_FM_SERVE_EXIT", "1"),
            ("FAKE_FM_ANSWER", "Pela via alternativa [1]."),
            ("FAKE_FM_RECORD", &record_s),
        ],
    );
    let (tokens, on_token) = collector();
    let g = fm
        .generate(&request(), &on_token, CancelFlag::default())
        .await
        .unwrap();
    assert_eq!(g.text, "Pela via alternativa [1].");
    assert_eq!(g.finish, FinishReason::Completed);
    assert!(!tokens.lock().unwrap().is_empty());
    let line: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(&record)
            .unwrap()
            .lines()
            .last()
            .unwrap(),
    )
    .unwrap();
    let args: Vec<&str> = line["respond"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    // Instructions travel in -i, separate from the prompt.
    let i = args.iter().position(|a| *a == "-i").unwrap();
    assert_eq!(args[i + 1], "Responda com base nos trechos.");
    assert_eq!(args[i + 2], "<pergunta>\nQual a carência?\n</pergunta>");

    // Refusal and cancellation through the same path.
    let (fm, _) = provider(
        "fallback-refuse",
        &[("FAKE_FM_SERVE_EXIT", "1"), ("FAKE_FM_MODE", "refuse")],
    );
    assert!(matches!(
        fm.generate(&request(), &|_| {}, CancelFlag::default())
            .await,
        Err(LlmError::Refused(_))
    ));
    let (fm, _) = provider(
        "fallback-cancel",
        &[
            ("FAKE_FM_SERVE_EXIT", "1"),
            ("FAKE_FM_ANSWER", "um dois três quatro cinco seis"),
            ("FAKE_FM_DELAY_MS", "150"),
        ],
    );
    let cancel = CancelFlag::default();
    let trigger = cancel.clone();
    let g = fm
        .generate(
            &request(),
            &move |t: &str| {
                if t.contains("dois") {
                    trigger.cancel()
                }
            },
            cancel,
        )
        .await
        .unwrap();
    assert_eq!(g.finish, FinishReason::Cancelled);
    assert!(!g.text.contains("seis"), "{:?}", g.text);
}

#[tokio::test]
async fn unavailable_reasons_are_classified() {
    let (fm, _) = provider(
        "not-enabled",
        &[("FAKE_FM_AVAILABLE", "Apple Intelligence is not enabled")],
    );
    assert_eq!(
        fm.status().await,
        LanguageModelStatus::Unavailable {
            kind: UnavailableKind::AppleIntelligenceDisabled,
            reason: "Apple Intelligence is not enabled".into()
        }
    );
    let (fm, _) = provider(
        "not-ready",
        &[(
            "FAKE_FM_AVAILABLE",
            "System model unavailable: modelNotReady",
        )],
    );
    assert!(fm.status().await.is_transient());
}

#[tokio::test]
async fn serve_exit_69_means_license_required() {
    let (fm, _) = provider("license", &[("FAKE_FM_SERVE_EXIT", "69")]);
    let err = fm
        .generate(&request(), &|_| {}, CancelFlag::default())
        .await
        .unwrap_err();
    assert_eq!(err, LlmError::LicenseRequired);
}

#[tokio::test]
async fn status_and_token_count() {
    let (fm, _) = provider("count", &[("FAKE_FM_TOKENS", "321")]);
    assert_eq!(fm.status().await, LanguageModelStatus::Available);
    assert_eq!(fm.count_tokens(&request()).await.unwrap(), 321);
    let (fm, _) = provider("nolicense", &[("FAKE_FM_AVAILABLE", "license")]);
    assert_eq!(fm.status().await, LanguageModelStatus::LicenseRequired);
}

#[tokio::test]
async fn a_dead_server_is_restarted_and_shutdown_removes_the_socket() {
    let (fm, dir) = provider("restart", &[]);
    fm.generate(&request(), &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    let pid: u32 = std::fs::read_to_string(dir.join("run/fm.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    std::process::Command::new("/bin/kill")
        .args(["-KILL", &pid.to_string()])
        .status()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    fm.generate(&request(), &|_| {}, CancelFlag::default())
        .await
        .expect("restarted");
    let new_pid: u32 = std::fs::read_to_string(dir.join("run/fm.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_ne!(pid, new_pid);
    fm.shutdown().await;
    assert!(!dir.join("run/fm.pid").exists());
}

#[test]
fn socket_path_fits_sun_path() {
    let config = FoundationModelsConfig::new(FAKE, "/tmp/run");
    assert!(config.socket_path().as_os_str().len() <= MAX_SOCKET_PATH);
    let mut long = config.clone();
    long.socket_dir = PathBuf::from("/tmp").join("x".repeat(120));
    assert!(long.socket_path().as_os_str().len() <= MAX_SOCKET_PATH);
}

#[tokio::test]
async fn honours_the_llm_provider_contract() {
    use nlmx_testing::{CONTRACT_ANSWER, LlmScenario, llm_provider_contract};
    llm_provider_contract(|scenario| {
        let env: Vec<(&str, &str)> = match scenario {
            LlmScenario::Answer => vec![("FAKE_FM_ANSWER", CONTRACT_ANSWER)],
            LlmScenario::Slow => vec![
                ("FAKE_FM_ANSWER", CONTRACT_ANSWER),
                ("FAKE_FM_DELAY_MS", "100"),
            ],
            LlmScenario::Refuse => vec![("FAKE_FM_MODE", "refuse")],
        };
        Arc::new(provider(&format!("contract-{scenario:?}"), &env).0)
    })
    .await;
}
