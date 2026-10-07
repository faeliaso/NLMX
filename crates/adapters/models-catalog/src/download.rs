//! Streaming download into `<file>.part` with resume (HTTP Range), incremental SHA-256, progress,
//! cancellation and periodic free-space checks.

use std::{
    io::Read,
    path::Path,
    time::{Duration, Instant},
};

use nlmx_application::ports::{CancelFlag, ProgressCallback};
use nlmx_domain::models::{DownloadProgress, ModelDescriptor, ModelError};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::{disk, disk::DiskMeter, store::io};

const PROGRESS_EVERY: Duration = Duration::from_millis(250);
const DISK_CHECK_EVERY: u64 = 64 * 1024 * 1024;

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Hashes the bytes already in `part` (to resume) on a blocking thread.
async fn hash_existing(part: &Path) -> Result<(Sha256, u64), ModelError> {
    let part = part.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut hasher = Sha256::new();
        let Ok(mut file) = std::fs::File::open(&part) else {
            return Ok((hasher, 0));
        };
        let mut buffer = vec![0u8; 1024 * 1024];
        let mut total = 0u64;
        loop {
            let n = file.read(&mut buffer).map_err(io)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            total += n as u64;
        }
        Ok((hasher, total))
    })
    .await
    .map_err(io)?
}

/// Downloads `model` into `part`, returning the SHA-256 of the complete file. On cancellation or a
/// network failure the partial file is kept so the next call resumes it.
pub async fn fetch(
    client: &reqwest::Client,
    model: &ModelDescriptor,
    part: &Path,
    disk_meter: &DiskMeter,
    progress: &ProgressCallback,
    cancel: &CancelFlag,
) -> Result<String, ModelError> {
    if let Some(dir) = part.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(io)?;
    }
    let (mut hasher, mut received) = hash_existing(part).await?;
    if received > model.size {
        tokio::fs::remove_file(part).await.map_err(io)?;
        (hasher, received) = (Sha256::new(), 0);
    }

    if received < model.size {
        let mut request = client.get(&model.url);
        if received > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={received}-"));
        }
        let mut response = request
            .send()
            .await
            .map_err(|err| ModelError::Network(err.to_string()))?;
        let status = response.status();
        let append = match status {
            reqwest::StatusCode::PARTIAL_CONTENT => true,
            // The server ignored the Range header: start over.
            reqwest::StatusCode::OK => {
                (hasher, received) = (Sha256::new(), 0);
                false
            }
            other => return Err(ModelError::Network(format!("o servidor respondeu {other}"))),
        };
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(part)
            .await
            .map_err(io)?;

        let started = Instant::now();
        let resumed_from = received;
        let mut last_report = Instant::now() - PROGRESS_EVERY;
        let mut next_disk_check = received + DISK_CHECK_EVERY;
        let report = |received: u64, force: bool, last: &mut Instant| {
            if force || last.elapsed() >= PROGRESS_EVERY {
                let elapsed = started.elapsed().as_secs_f64().max(0.001);
                progress(DownloadProgress {
                    received,
                    total: model.size,
                    bytes_per_second: (received - resumed_from) as f64 / elapsed,
                });
                *last = Instant::now();
            }
        };
        report(received, true, &mut last_report);

        loop {
            if cancel.is_cancelled() {
                file.flush().await.map_err(io)?;
                return Err(ModelError::Cancelled);
            }
            let chunk = match response.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(err) => {
                    file.flush().await.map_err(io)?;
                    return Err(ModelError::Network(format!("conexão interrompida: {err}")));
                }
            };
            file.write_all(&chunk).await.map_err(io)?;
            hasher.update(&chunk);
            received += chunk.len() as u64;
            if received >= next_disk_check {
                next_disk_check = received + DISK_CHECK_EVERY;
                let remaining = model.size.saturating_sub(received);
                let available = disk_meter(part).map_err(io)?;
                if available < remaining + disk::margin(model.size) / 4 {
                    file.flush().await.map_err(io)?;
                    return Err(ModelError::InsufficientSpace {
                        required: remaining,
                        available,
                    });
                }
            }
            report(received, false, &mut last_report);
        }
        file.flush().await.map_err(io)?;
        file.sync_all().await.map_err(io)?;
        report(received, true, &mut last_report);
    }

    if received != model.size {
        return Err(ModelError::Network(format!(
            "download incompleto: {received} de {} bytes",
            model.size
        )));
    }
    let actual = hex(&hasher.finalize());
    if actual != model.sha256 {
        let _ = tokio::fs::remove_file(part).await;
        return Err(ModelError::ChecksumMismatch {
            expected: model.sha256.clone(),
            actual,
        });
    }
    Ok(actual)
}

/// Full SHA-256 of a file, on a blocking thread.
pub async fn sha256_file(path: &Path) -> Result<String, ModelError> {
    let (hasher, _) = hash_existing(path).await?;
    Ok(hex(&hasher.finalize()))
}
