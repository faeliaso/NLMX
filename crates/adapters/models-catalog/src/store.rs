//! On-disk layout: `<root>/<id>/<version>/<file>` + `manifest.json`, partial downloads as `.part`.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use nlmx_domain::models::{InstalledModel, ModelDescriptor, ModelError};
use serde::{Deserialize, Serialize};

pub const MANIFEST: &str = "manifest.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub version: String,
    pub file: String,
    pub size: u64,
    pub sha256: String,
    pub source_url: String,
    pub installed_at: String,
    #[serde(default)]
    pub verified_at: Option<String>,
}

pub struct Store {
    root: PathBuf,
}

pub fn io(err: impl std::fmt::Display) -> ModelError {
    ModelError::Io(err.to_string())
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn model_dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    pub fn version_dir(&self, id: &str, version: &str) -> PathBuf {
        self.model_dir(id).join(version)
    }

    pub fn file_path(&self, model: &ModelDescriptor) -> PathBuf {
        self.version_dir(&model.id, &model.version)
            .join(&model.file_name)
    }

    pub fn part_path(&self, model: &ModelDescriptor) -> PathBuf {
        self.version_dir(&model.id, &model.version)
            .join(format!("{}.part", model.file_name))
    }

    /// Installed versions of a model with their directories, newest installation first.
    pub fn manifests(&self, id: &str) -> Vec<(Manifest, PathBuf)> {
        let Ok(entries) = fs::read_dir(self.model_dir(id)) else {
            return Vec::new();
        };
        let mut found: Vec<(Manifest, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let dir = entry.path();
                let manifest: Manifest =
                    serde_json::from_str(&fs::read_to_string(dir.join(MANIFEST)).ok()?).ok()?;
                Some((manifest, dir))
            })
            .collect();
        found.sort_by(|a, b| b.0.installed_at.cmp(&a.0.installed_at));
        found
    }

    pub fn write_manifest(&self, dir: &Path, manifest: &Manifest) -> Result<(), ModelError> {
        let tmp = dir.join(format!("{MANIFEST}.tmp"));
        fs::write(
            &tmp,
            serde_json::to_vec_pretty(manifest).expect("serializable"),
        )
        .map_err(io)?;
        fs::rename(&tmp, dir.join(MANIFEST)).map_err(io)
    }

    pub fn installed(manifest: &Manifest, dir: &Path) -> InstalledModel {
        InstalledModel {
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            path: dir.join(&manifest.file).display().to_string(),
            size: manifest.size,
            sha256: manifest.sha256.clone(),
            installed_at: manifest.installed_at.clone(),
            verified_at: manifest.verified_at.clone(),
        }
    }

    /// Bytes used under the root (installed files and partial downloads).
    pub fn usage(&self) -> u64 {
        fn walk(path: &Path) -> u64 {
            match fs::symlink_metadata(path) {
                Ok(m) if m.is_dir() => fs::read_dir(path)
                    .map(|entries| entries.flatten().map(|e| walk(&e.path())).sum())
                    .unwrap_or(0),
                Ok(m) => m.len(),
                Err(_) => 0,
            }
        }
        walk(&self.root)
    }
}

/// Cheap integrity check: file present, size as recorded, GGUF magic header.
pub fn quick_check(manifest: &Manifest, dir: &Path) -> Result<(), String> {
    let path = dir.join(&manifest.file);
    let len = fs::metadata(&path)
        .map_err(|_| "arquivo do modelo ausente".to_string())?
        .len();
    if len != manifest.size {
        return Err(format!("tamanho {len} bytes, esperado {}", manifest.size));
    }
    let mut magic = [0u8; 4];
    fs::File::open(&path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .map_err(|err| format!("não foi possível ler o arquivo: {err}"))?;
    if &magic != b"GGUF" {
        return Err("o arquivo não é um GGUF válido".into());
    }
    Ok(())
}

/// ISO-8601 UTC timestamp (seconds precision) without external crates.
pub fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}
