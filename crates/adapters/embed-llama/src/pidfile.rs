//! Records the running server so a later app session can find it (reuse or clean up).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PidRecord {
    pub pid: u32,
    pub port: u16,
    pub binary: PathBuf,
    pub model_path: PathBuf,
}

pub fn read(path: &Path) -> Option<PidRecord> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

pub fn write(path: &Path, record: &PidRecord) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(
        path,
        serde_json::to_vec_pretty(record).expect("serializable"),
    )
}

pub fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// Full command line of a live process, or `None` if it does not exist.
pub fn process_command(pid: u32) -> Option<String> {
    // `-ww`: never truncate; `stat` lets zombies (exited, not yet reaped) count as gone.
    let output = std::process::Command::new("/bin/ps")
        .args([
            "-ww",
            "-p",
            &pid.to_string(),
            "-o",
            "stat=",
            "-o",
            "command=",
        ])
        .output()
        .ok()?;
    let line = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let (stat, command) = line.split_once(char::is_whitespace)?;
    (output.status.success() && !stat.starts_with('Z')).then(|| command.trim().to_string())
}

/// Whether `pid` is alive and is `binary` (so we never signal someone else's process).
pub fn is_our_server(pid: u32, binary: &Path) -> bool {
    process_command(pid).is_some_and(|command| command.starts_with(&binary.display().to_string()))
}

pub fn signal(pid: u32, signal: &str) {
    let _ = std::process::Command::new("/bin/kill")
        .args([&format!("-{signal}"), &pid.to_string()])
        .status();
}

pub fn key_path(run_dir: &Path) -> PathBuf {
    run_dir.join("llama-server.key")
}
