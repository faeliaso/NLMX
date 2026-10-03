//! Stand-in for `/usr/bin/fm` in tests: `available`, `count-tokens -q -i <s> <u>`,
//! `respond --stream -i <s> <u>` and `serve --socket <path>` (health + streamed chat
//! completions). Behaviour via environment:
//! `FAKE_FM_AVAILABLE=license` (exit 69) or `=<message>` (exit 1 with it on stderr),
//! `FAKE_FM_TOKENS=<n>`, `FAKE_FM_SERVE_EXIT=<code>`, `FAKE_FM_MODE=answer|refuse|length`,
//! `FAKE_FM_ANSWER=<text>`, `FAKE_FM_DELAY_MS=<ms>`, `FAKE_FM_RECORD=<file>` (appends each
//! request as a JSON line: the serve body, or `{"respond": [args]}`).

use std::{io::Write, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("available") => match env("FAKE_FM_AVAILABLE").as_deref() {
            Some("license") => std::process::exit(69),
            Some(message) => {
                eprintln!("Error: {message}");
                std::process::exit(1);
            }
            None => println!("System model available"),
        },
        Some("respond") => respond(&args[1..]).await,
        Some("count-tokens") => {
            let texts: Vec<&String> = args
                .iter()
                .skip(1)
                .filter(|a| !a.starts_with('-'))
                .collect();
            let count = env("FAKE_FM_TOKENS")
                .and_then(|t| t.parse().ok())
                .unwrap_or_else(|| texts.iter().map(|t| t.chars().count()).sum::<usize>() / 4);
            println!("{count}");
        }
        Some("serve") => {
            if let Some(code) = env("FAKE_FM_SERVE_EXIT").and_then(|c| c.parse().ok()) {
                std::process::exit(code);
            }
            let socket = args
                .iter()
                .position(|a| a == "--socket")
                .and_then(|i| args.get(i + 1))
                .expect("--socket <path>");
            let _ = std::fs::remove_file(socket);
            let listener = UnixListener::bind(socket).expect("bind");
            eprintln!("listening on {socket}");
            loop {
                let (stream, _) = listener.accept().await.expect("accept");
                tokio::spawn(handle(stream));
            }
        }
        _ => std::process::exit(64),
    }
}

fn record(line: &str) {
    if let Some(path) = env("FAKE_FM_RECORD") {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

fn answer() -> String {
    env("FAKE_FM_ANSWER").unwrap_or_else(|| "Resposta de teste [1].".into())
}

fn delay() -> Duration {
    Duration::from_millis(
        env("FAKE_FM_DELAY_MS")
            .and_then(|d| d.parse().ok())
            .unwrap_or(0),
    )
}

/// `fm respond --stream`: plain text on stdout, "Error: ..." on stderr with exit 1.
async fn respond(args: &[String]) {
    record(&serde_json::json!({ "respond": args }).to_string());
    if env("FAKE_FM_MODE").as_deref() == Some("refuse") {
        eprintln!("Error: The model's safety guardrails were triggered.");
        std::process::exit(1);
    }
    let mut out = std::io::stdout();
    for piece in answer().split_inclusive(' ') {
        if out
            .write_all(piece.as_bytes())
            .and_then(|_| out.flush())
            .is_err()
        {
            return;
        }
        if !delay().is_zero() {
            tokio::time::sleep(delay()).await;
        }
    }
    let _ = writeln!(out);
}

async fn handle(mut stream: UnixStream) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let (head, body) = loop {
        let n = stream.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..end]).to_string();
            let length = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse().ok())?
                })
                .unwrap_or(0usize);
            while buf.len() < end + 4 + length {
                let n = stream.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            break (head, buf[end + 4..].to_vec());
        }
    };
    let request_line = head.lines().next().unwrap_or_default().to_string();
    if request_line.starts_with("GET /health") {
        let body =
            r#"{"models":[{"available":true,"name":"system"}],"status":"fm serve is running"}"#;
        let _ = stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await;
        return;
    }
    if !request_line.starts_with("POST /v1/chat/completions") {
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
            .await;
        return;
    }
    if let Some(path) = env("FAKE_FM_RECORD") {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{}", String::from_utf8_lossy(&body));
        }
    }
    let _ = stream
        .write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
        )
        .await;
    let send = |data: String| format!("data: {data}\n\n");
    let delta = |content: &str| {
        serde_json::json!({"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":content}}]}).to_string()
    };
    let _ = stream
        .write_all(
            send(r#"{"choices":[{"index":0,"delta":{"role":"assistant"}}]}"#.into()).as_bytes(),
        )
        .await;
    let mode = env("FAKE_FM_MODE").unwrap_or_else(|| "answer".into());
    if mode == "refuse" {
        let _ = stream
            .write_all(b"event: error\ndata: {\"error\":{\"code\":\"500\",\"message\":\"The model's safety guardrails were triggered.\",\"type\":\"server_error\"}}\n\n")
            .await;
        return;
    }
    let delay = Duration::from_millis(
        env("FAKE_FM_DELAY_MS")
            .and_then(|d| d.parse().ok())
            .unwrap_or(0),
    );
    let answer = env("FAKE_FM_ANSWER").unwrap_or_else(|| "Resposta de teste [1].".into());
    for piece in answer.split_inclusive(' ') {
        if stream
            .write_all(send(delta(piece)).as_bytes())
            .await
            .is_err()
        {
            return; // client went away (cancelled)
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    let reason = if mode == "length" { "length" } else { "stop" };
    let finish =
        serde_json::json!({"choices":[{"index":0,"delta":{},"finish_reason":reason}]}).to_string();
    let _ = stream.write_all(send(finish).as_bytes()).await;
    let _ = stream.write_all(b"data: [DONE]\n\n").await;
}
