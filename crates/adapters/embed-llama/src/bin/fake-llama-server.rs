//! A stand-in for `llama-server` used by this crate's tests. It accepts the same arguments the
//! supervisor passes, enforces the API key, and serves `/health`, `/v1/models` and
//! `/v1/embeddings` with deterministic, unnormalized vectors (returned in reverse order).
//!
//! Behaviour switches (environment):
//!   FAKE_LOAD_MS=n          /health answers 503 for the first n ms
//!   FAKE_NEVER_READY=1      /health always answers 503
//!   FAKE_EXIT_AT_START=1    logs an error and exits during startup
//!   FAKE_CRASH_AFTER=n      exits after answering n embedding requests
//!   FAKE_SLOW_MS=n          waits n ms before answering embeddings
//!   FAKE_DIMS=n             embedding dimension (default 8)

use std::{
    env,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

struct Settings {
    api_key: String,
    model: String,
    dims: usize,
    started: Instant,
    load: Duration,
    never_ready: bool,
    crash_after: Option<usize>,
    slow: Duration,
    served: AtomicUsize,
}

fn env_ms(name: &str) -> Duration {
    Duration::from_millis(
        env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
    )
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let host = arg(&args, "--host").unwrap_or_else(|| "127.0.0.1".into());
    let port = arg(&args, "--port").expect("--port");
    let key_file = arg(&args, "--api-key-file").expect("--api-key-file");
    let model = arg(&args, "--model").expect("--model");
    eprintln!("fake: loading model {model}");

    if env::var("FAKE_EXIT_AT_START").is_ok() {
        std::thread::sleep(Duration::from_millis(150));
        eprintln!("fake: error: failed to load model '{model}'");
        std::process::exit(1);
    }

    let settings = Arc::new(Settings {
        api_key: std::fs::read_to_string(&key_file)
            .expect("key file")
            .trim()
            .to_string(),
        model,
        dims: env::var("FAKE_DIMS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8),
        started: Instant::now(),
        load: env_ms("FAKE_LOAD_MS"),
        never_ready: env::var("FAKE_NEVER_READY").is_ok(),
        crash_after: env::var("FAKE_CRASH_AFTER")
            .ok()
            .and_then(|v| v.parse().ok()),
        slow: env_ms("FAKE_SLOW_MS"),
        served: AtomicUsize::new(0),
    });

    let listener = TcpListener::bind(format!("{host}:{port}")).expect("bind");
    eprintln!("fake: server is listening on http://{host}:{port}");
    for stream in listener.incoming().flatten() {
        let settings = Arc::clone(&settings);
        std::thread::spawn(move || handle(stream, &settings));
    }
}

fn handle(mut stream: TcpStream, s: &Settings) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut auth = String::new();
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() || header == "\r\n" || header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').unwrap_or((&header, ""));
        match name.to_ascii_lowercase().as_str() {
            "authorization" => auth = value.trim().to_string(),
            "content-length" => length = value.trim().parse().unwrap_or(0),
            _ => {}
        }
    }
    let mut body = vec![0u8; length];
    let _ = reader.read_exact(&mut body);

    let path = request_line.split_whitespace().nth(1).unwrap_or("/");
    let authorized = auth == format!("Bearer {}", s.api_key);
    let (status, json) = match path {
        "/health" => {
            if s.never_ready || s.started.elapsed() < s.load {
                (503, r#"{"error":{"message":"Loading model"}}"#.to_string())
            } else {
                (200, r#"{"status":"ok"}"#.to_string())
            }
        }
        _ if !authorized => (401, r#"{"error":{"message":"Invalid API Key"}}"#.to_string()),
        "/v1/models" => (
            200,
            serde_json::json!({ "data": [{ "id": s.model, "meta": { "n_embd": s.dims, "n_ctx_train": 4096, "n_params": 1234 } }] })
                .to_string(),
        ),
        "/v1/embeddings" => {
            std::thread::sleep(s.slow);
            embeddings(&body, s.dims)
        }
        _ => (404, r#"{"error":{"message":"not found"}}"#.to_string()),
    };
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        503 => "Service Unavailable",
        400 => "Bad Request",
        _ => "Not Found",
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
        json.len()
    );
    let _ = stream.flush();

    if path == "/v1/embeddings" && status == 200 {
        let served = s.served.fetch_add(1, Ordering::SeqCst) + 1;
        if s.crash_after.is_some_and(|limit| served >= limit) {
            eprintln!("fake: simulated crash after {served} requests");
            std::process::exit(3);
        }
    }
}

fn embeddings(body: &[u8], dims: usize) -> (u16, String) {
    let Ok(request) = serde_json::from_slice::<serde_json::Value>(body) else {
        return (400, r#"{"error":{"message":"invalid json"}}"#.to_string());
    };
    let inputs: Vec<String> = match &request["input"] {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => return (400, r#"{"error":{"message":"input required"}}"#.to_string()),
    };
    if inputs.iter().any(|i| i.contains("TOO_LONG")) {
        return (400, r#"{"error":{"message":"input is too large to process. increase the physical batch size"}}"#.to_string());
    }
    // Reverse order on purpose: clients must sort by index.
    let data: Vec<serde_json::Value> = inputs
        .iter()
        .enumerate()
        .rev()
        .map(|(index, text)| serde_json::json!({ "index": index, "object": "embedding", "embedding": vector(text, dims) }))
        .collect();
    (
        200,
        serde_json::json!({ "object": "list", "data": data }).to_string(),
    )
}

/// Deterministic per text, unnormalized (magnitude varies with the text).
fn vector(text: &str, dims: usize) -> Vec<f32> {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.bytes() {
        hash = (hash ^ byte as u64).wrapping_mul(0x100000001b3);
    }
    (0..dims)
        .map(|i| {
            let x = hash.rotate_left(i as u32 * 7) ^ (i as u64).wrapping_mul(0x9E3779B97F4A7C15);
            ((x % 2001) as f32 - 1000.0) * (1.0 + text.len() as f32)
        })
        .collect()
}
