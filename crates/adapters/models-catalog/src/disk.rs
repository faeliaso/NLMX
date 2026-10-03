//! Free-space checks, injectable for tests.

use std::{path::Path, sync::Arc};

/// Returns the free bytes on the volume holding `path`.
pub type DiskMeter = Arc<dyn Fn(&Path) -> std::io::Result<u64> + Send + Sync>;

pub fn system() -> DiskMeter {
    Arc::new(|path: &Path| {
        // The directory may not exist yet: measure the nearest existing ancestor.
        let existing = path
            .ancestors()
            .find(|p| p.exists())
            .unwrap_or(Path::new("/"));
        fs4::available_space(existing)
    })
}

/// Free space to keep on top of the bytes still to download.
pub fn margin(size: u64) -> u64 {
    (size / 20).max(256 * 1024 * 1024)
}
