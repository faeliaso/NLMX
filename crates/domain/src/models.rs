//! Runtime (bundled inference engine) and Model (GGUF files downloaded on demand) concepts.

use std::fmt;

/// The inference engine shipped inside the app. It is detected and versioned, never downloaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeInfo {
    pub name: String,
    /// e.g. "b11349".
    pub build: String,
    pub commit: Option<String>,
    pub path: String,
    /// Inside the app bundle (as opposed to a development or override location).
    pub bundled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct License {
    pub id: String,
    pub url: String,
}

/// A downloadable model from the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDescriptor {
    pub id: String,
    pub display_name: String,
    pub description: String,
    /// Pinned upstream revision; a different value in a newer catalog means an update.
    pub version: String,
    pub url: String,
    pub file_name: String,
    pub size: u64,
    pub sha256: String,
    pub license: License,
    pub languages: Vec<String>,
    pub dimensions: u32,
    pub pooling: String,
    pub query_prefix: String,
    pub passage_prefix: String,
    pub context_size: u32,
    pub recommended: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledModel {
    pub id: String,
    pub version: String,
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub installed_at: String,
    /// Last full checksum verification, if any.
    pub verified_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelState {
    NotInstalled,
    /// A resumable download exists.
    PartiallyDownloaded {
        bytes: u64,
        total: u64,
    },
    Installed {
        version: String,
    },
    UpdateAvailable {
        installed: String,
        latest: String,
    },
    Corrupted {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskSpace {
    /// Bytes used by installed models and partial downloads.
    pub models_bytes: u64,
    /// Free bytes on the volume holding the models directory.
    pub available_bytes: u64,
}

/// What a download will do, shown to the user before they confirm it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadPlan {
    pub model: ModelDescriptor,
    /// Bytes already present from an interrupted download.
    pub resume_from: u64,
    /// Free space needed to finish, including a safety margin.
    pub required_bytes: u64,
    pub available_bytes: u64,
    /// Installed version this download replaces (an update).
    pub replaces: Option<String>,
}

impl DownloadPlan {
    pub fn fits_on_disk(&self) -> bool {
        self.available_bytes >= self.required_bytes
    }

    /// The only way to obtain a [`ConfirmedDownload`]: call this after the user agrees to the
    /// plan (size, license, disk usage). Nothing in the app may call it on the user's behalf.
    pub fn confirm(self) -> ConfirmedDownload {
        ConfirmedDownload { plan: self }
    }
}

/// A download the user explicitly agreed to. Cannot be constructed outside [`DownloadPlan::confirm`]:
///
/// ```compile_fail
/// # fn plan() -> nlmx_domain::models::DownloadPlan { unimplemented!() }
/// // The field is private: no confirmation can be made up without the plan's `confirm()`.
/// let _ = nlmx_domain::models::ConfirmedDownload { plan: plan() };
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedDownload {
    plan: DownloadPlan,
}

impl ConfirmedDownload {
    pub fn plan(&self) -> &DownloadPlan {
        &self.plan
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DownloadProgress {
    pub received: u64,
    pub total: u64,
    pub bytes_per_second: f64,
}

impl DownloadProgress {
    pub fn fraction(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.received as f64 / self.total as f64
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    pub model: InstalledModel,
    pub ok: bool,
    pub actual_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    UnknownModel(String),
    NotInstalled(String),
    AlreadyInstalled(String),
    InsufficientSpace { required: u64, available: u64 },
    ChecksumMismatch { expected: String, actual: String },
    Network(String),
    Cancelled,
    Io(String),
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let gb = |b: &u64| *b as f64 / 1_000_000_000.0;
        match self {
            Self::UnknownModel(id) => write!(f, "modelo desconhecido: {id}"),
            Self::NotInstalled(id) => write!(f, "modelo não instalado: {id}"),
            Self::AlreadyInstalled(id) => write!(f, "o modelo {id} já está instalado nesta versão"),
            Self::InsufficientSpace {
                required,
                available,
            } => write!(
                f,
                "espaço em disco insuficiente: são necessários {:.1} GB e há {:.1} GB livres",
                gb(required),
                gb(available)
            ),
            Self::ChecksumMismatch { expected, actual } => {
                write!(
                    f,
                    "o arquivo baixado está corrompido (SHA-256 {actual}, esperado {expected})"
                )
            }
            Self::Network(m) => write!(f, "falha no download: {m}"),
            Self::Cancelled => f.write_str("download cancelado"),
            Self::Io(m) => write!(f, "erro de arquivo: {m}"),
        }
    }
}

impl std::error::Error for ModelError {}
