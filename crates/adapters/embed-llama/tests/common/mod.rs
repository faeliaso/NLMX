//! Shared helpers: a temp data dir and a supervisor configured for the fake server.

#![allow(dead_code)]

use std::{path::PathBuf, time::Duration};

use nlmx_embed_llama::LlamaServerConfig;

pub const FAKE: &str = env!("CARGO_BIN_EXE_fake-llama-server");

pub fn data_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nlmx-llama-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A config for the fake server with a dummy model file and short timeouts.
pub fn fake_config(dir: &std::path::Path, env: &[(&str, &str)]) -> LlamaServerConfig {
    let model = dir.join("model.gguf");
    if !model.exists() {
        std::fs::write(&model, b"GGUF fake").unwrap();
    }
    let mut config = LlamaServerConfig::new(PathBuf::from(FAKE), model, dir);
    config.startup_timeout = Duration::from_secs(5);
    config.stop_timeout = Duration::from_secs(2);
    config.idle_shutdown = None;
    config.extra_env = env
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    config
}

/// Alive and not a zombie (test processes are children of the test binary, so killed ones
/// linger as zombies until reaped).
pub fn alive(pid: u32) -> bool {
    std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
        .is_ok_and(|o| {
            o.status.success() && !String::from_utf8_lossy(&o.stdout).trim().starts_with('Z')
        })
}

pub async fn wait_dead(pid: u32) -> bool {
    for _ in 0..50 {
        if !alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}
