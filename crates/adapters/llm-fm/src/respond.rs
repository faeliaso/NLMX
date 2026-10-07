//! Fallback generation through `fm respond --stream` (one process per request), used when
//! `fm serve` can't be started. Same contract: instructions in `-i`, context in the prompt; earlier
//! turns, which `fm respond` can't take, are written into the prompt.

use std::{process::Stdio, time::Instant};

use nlmx_application::ports::CancelFlag;
use nlmx_domain::generation::{FinishReason, Generation, GenerationRequest, LlmError};
use tokio::{io::AsyncReadExt, process::Command};

use crate::{CANCEL_POLL, FoundationModelsConfig, errors};

pub async fn generate(
    config: &FoundationModelsConfig,
    request: &GenerationRequest,
    on_token: &(dyn Fn(&str) + Send + Sync),
    cancel: CancelFlag,
) -> Result<Generation, LlmError> {
    let mut command = Command::new(&config.binary);
    command.args(["respond", "--stream", "--model", "system"]);
    if request.temperature <= 0.0 {
        command.arg("--greedy");
    }
    command
        .arg("-i")
        .arg(&request.system)
        .arg(request.flat_user())
        .env("NO_COLOR", "1")
        .envs(config.extra_env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|e| LlmError::Unavailable(format!("não foi possível executar o fm: {e}")))?;
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let stderr_task = tokio::spawn(async move {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text).await;
        text
    });

    let mut text = String::new();
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = [0u8; 1024];
    let mut idle_since = Instant::now();
    loop {
        if cancel.is_cancelled() {
            let _ = child.kill().await;
            return Ok(Generation::new(
                text.trim_end().to_string(),
                FinishReason::Cancelled,
            ));
        }
        let n = match tokio::time::timeout(CANCEL_POLL, stdout.read(&mut buf)).await {
            Err(_) => {
                if idle_since.elapsed() > config.idle_timeout {
                    let _ = child.kill().await;
                    return Err(LlmError::Timeout);
                }
                continue;
            }
            Ok(Err(e)) => return Err(LlmError::Protocol(e.to_string())),
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => n,
        };
        idle_since = Instant::now();
        pending.extend_from_slice(&buf[..n]);
        // Emit only complete UTF-8 characters.
        let valid = match std::str::from_utf8(&pending) {
            Ok(s) => s.len(),
            Err(e) => e.valid_up_to(),
        };
        if valid > 0 {
            let piece = String::from_utf8_lossy(&pending[..valid]).into_owned();
            pending.drain(..valid);
            on_token(&piece);
            text.push_str(&piece);
        }
    }
    let status = child
        .wait()
        .await
        .map_err(|e| LlmError::Protocol(e.to_string()))?;
    let stderr = stderr_task.await.unwrap_or_default();
    if !status.success() {
        return Err(errors::from_exit(status.code(), &stderr));
    }
    Ok(Generation::new(
        text.trim_end().to_string(),
        FinishReason::Completed,
    ))
}
