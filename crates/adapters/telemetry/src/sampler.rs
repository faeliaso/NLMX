//! Memory (RSS of the app and of its helper processes) and storage (database, library,
//! models, logs) sampling. Recorded as measurements like everything else.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use nlmx_application::telemetry::record;
use nlmx_domain::telemetry::Measurement;

/// Where the app keeps its data.
#[derive(Debug, Clone)]
pub struct DataLayout {
    pub database: PathBuf,
    pub library: PathBuf,
    pub models: PathBuf,
    pub logs: PathBuf,
    /// Pid files of helper processes (`llama-server.json`, `fm.pid`).
    pub run: PathBuf,
}

/// Resident memory of a process, in bytes (`ps` reports KiB).
pub fn rss_bytes(pid: u32) -> Option<u64> {
    let output = Command::new("/bin/ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let kib: u64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .ok()?;
    Some(kib * 1024)
}

/// Total size of the files under `path` (a file or a directory).
pub fn size_of(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    if !meta.is_dir() {
        return 0;
    }
    fs::read_dir(path)
        .map(|entries| entries.flatten().map(|e| size_of(&e.path())).sum())
        .unwrap_or(0)
}

fn llama_pid(run: &Path) -> Option<u32> {
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(run.join("llama-server.json")).ok()?).ok()?;
    json.get("pid")?.as_u64().map(|p| p as u32)
}

fn fm_pid(run: &Path) -> Option<u32> {
    fs::read_to_string(run.join("fm.pid"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

impl DataLayout {
    pub fn sample_resources(&self) -> Measurement {
        Measurement::Resources {
            rss_bytes: rss_bytes(std::process::id()).unwrap_or(0),
            llama_rss_bytes: llama_pid(&self.run).and_then(rss_bytes),
            fm_rss_bytes: fm_pid(&self.run).and_then(rss_bytes),
        }
    }

    pub fn sample_storage(&self) -> Measurement {
        let database = size_of(&self.database)
            + ["-wal", "-shm"]
                .iter()
                .map(|suffix| {
                    let mut p = self.database.clone().into_os_string();
                    p.push(suffix);
                    size_of(Path::new(&p))
                })
                .sum::<u64>();
        Measurement::Storage {
            database_bytes: database,
            library_bytes: size_of(&self.library),
            models_bytes: size_of(&self.models),
            logs_bytes: size_of(&self.logs),
        }
    }

    /// Samples both and records them.
    pub fn sample(&self) {
        record(&self.sample_resources());
        record(&self.sample_storage());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_this_process_and_directory_sizes() {
        assert!(rss_bytes(std::process::id()).unwrap() > 1_000_000);
        let dir = std::env::temp_dir().join(format!("nlmx-sampler-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("library/sub")).unwrap();
        fs::write(dir.join("library/a.pdf"), vec![0u8; 1000]).unwrap();
        fs::write(dir.join("library/sub/b.pdf"), vec![0u8; 500]).unwrap();
        fs::write(dir.join("db.sqlite3"), vec![0u8; 300]).unwrap();
        fs::write(dir.join("db.sqlite3-wal"), vec![0u8; 20]).unwrap();
        fs::create_dir_all(dir.join("run")).unwrap();
        fs::write(dir.join("run/fm.pid"), std::process::id().to_string()).unwrap();
        let layout = DataLayout {
            database: dir.join("db.sqlite3"),
            library: dir.join("library"),
            models: dir.join("models"),
            logs: dir.join("logs"),
            run: dir.join("run"),
        };
        assert_eq!(
            layout.sample_storage(),
            Measurement::Storage {
                database_bytes: 320,
                library_bytes: 1500,
                models_bytes: 0,
                logs_bytes: 0
            }
        );
        let Measurement::Resources {
            rss_bytes,
            llama_rss_bytes,
            fm_rss_bytes,
        } = layout.sample_resources()
        else {
            panic!()
        };
        assert!(rss_bytes > 0 && llama_rss_bytes.is_none() && fm_rss_bytes.is_some());
    }
}
