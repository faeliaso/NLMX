//! `FileStore`: hashes imported PDFs and copies them into the app library as `<sha256>.pdf`.

use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

use nlmx_application::ports::{BoxFuture, FileDigest, FileStore, StorageError};
use sha2::{Digest, Sha256};

/// Directory name of the library inside the app data directory.
pub const LIBRARY_DIR: &str = "library";

pub struct FsLibrary {
    dir: PathBuf,
}

impl FsLibrary {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn path_for(&self, sha256: &str) -> PathBuf {
        self.dir.join(format!("{sha256}.pdf"))
    }
}

fn error(action: &str, path: &Path, err: io::Error) -> StorageError {
    StorageError::new(format!(
        "Não foi possível {action} {}: {err}",
        path.display()
    ))
}

/// SHA-256 of a file, read in 64 KiB blocks.
pub fn digest_file(path: &Path) -> Result<FileDigest, StorageError> {
    let mut file = File::open(path).map_err(|err| error("ler", path, err))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|err| error("ler", path, err))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        size += n as u64;
    }
    let sha256 = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok(FileDigest { sha256, size })
}

fn store_file(dir: &Path, source: &Path, destination: &Path) -> Result<(), StorageError> {
    fs::create_dir_all(dir).map_err(|err| error("criar a biblioteca em", dir, err))?;
    let source_len = fs::metadata(source)
        .map_err(|err| error("ler", source, err))?
        .len();
    if fs::metadata(destination).is_ok_and(|m| m.len() == source_len) {
        return Ok(()); // Same content hash and size: already in the library.
    }
    // Copy to a temporary name first so a crash never leaves a truncated library file.
    let temporary = destination.with_extension(format!("pdf.tmp-{}", std::process::id()));
    fs::copy(source, &temporary).map_err(|err| error("copiar", source, err))?;
    fs::rename(&temporary, destination).map_err(|err| {
        let _ = fs::remove_file(&temporary);
        error("gravar", destination, err)
    })
}

fn remove_file(path: &Path) -> Result<(), StorageError> {
    match fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(error("apagar", path, err)),
        _ => Ok(()),
    }
}

/// Files changed more recently than this are left alone by `prune`: an import copies the file
/// into the library a moment before its document is recorded.
const PRUNE_GRACE_SECS: i64 = 10 * 60;

/// The hash of a library file name: `<sha>.pdf` or a leftover `<sha>.pdf.tmp-<pid>`.
fn library_hash(name: &str) -> Option<&str> {
    let sha = name.get(..64)?;
    let rest = &name[64..];
    let valid = sha.bytes().all(|b| b.is_ascii_hexdigit());
    (valid && (rest == ".pdf" || rest.starts_with(".pdf.tmp-"))).then_some(sha)
}

fn prune(
    dir: &Path,
    keep: &std::collections::HashSet<String>,
    grace_secs: i64,
) -> Result<u32, StorageError> {
    use std::os::unix::fs::MetadataExt;
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(error("ler a biblioteca em", dir, err)),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(sha) = name.to_str().and_then(library_hash) else {
            continue; // Not ours: never touched.
        };
        let recent = entry.metadata().is_ok_and(|m| now - m.ctime() < grace_secs);
        if keep.contains(sha) || recent {
            continue;
        }
        remove_file(&entry.path())?;
        removed += 1;
    }
    Ok(removed)
}

impl FileStore for FsLibrary {
    fn digest<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<FileDigest, StorageError>> {
        let path = path.to_path_buf();
        Box::pin(async move { blocking(move || digest_file(&path)).await })
    }

    fn store<'a>(
        &'a self,
        path: &'a Path,
        sha256: &'a str,
    ) -> BoxFuture<'a, Result<PathBuf, StorageError>> {
        let (dir, source, destination) =
            (self.dir.clone(), path.to_path_buf(), self.path_for(sha256));
        Box::pin(async move {
            blocking(move || store_file(&dir, &source, &destination).map(|()| destination)).await
        })
    }

    fn remove<'a>(&'a self, sha256: &'a str) -> BoxFuture<'a, Result<(), StorageError>> {
        let path = self.path_for(sha256);
        Box::pin(async move { blocking(move || remove_file(&path)).await })
    }

    fn prune(&self, keep: Vec<String>) -> BoxFuture<'_, Result<u32, StorageError>> {
        let dir = self.dir.clone();
        Box::pin(async move {
            blocking(move || prune(&dir, &keep.into_iter().collect(), PRUNE_GRACE_SECS)).await
        })
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, StorageError> + Send + 'static,
) -> Result<T, StorageError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|err| StorageError::new(format!("tarefa de arquivo falhou: {err}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nlmx-fs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn digests_files() {
        let dir = temp_dir("digest");
        let file = dir.join("abc.txt");
        fs::write(&file, "abc").unwrap();
        let digest = FsLibrary::new(dir.join("library"))
            .digest(&file)
            .await
            .unwrap();
        assert_eq!(
            digest.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(digest.size, 3);

        let big = dir.join("big.bin");
        fs::write(&big, vec![7u8; 200_000]).unwrap();
        assert_eq!(
            FsLibrary::new(&dir).digest(&big).await.unwrap().size,
            200_000
        );
    }

    #[tokio::test]
    async fn stores_by_hash_idempotently() {
        let dir = temp_dir("store");
        let source = dir.join("contrato.pdf");
        fs::write(&source, b"%PDF-1.7 fake").unwrap();
        let library = FsLibrary::new(dir.join(LIBRARY_DIR));
        let sha = library.digest(&source).await.unwrap().sha256;

        let stored = library.store(&source, &sha).await.unwrap();
        assert_eq!(stored, dir.join(LIBRARY_DIR).join(format!("{sha}.pdf")));
        assert_eq!(fs::read(&stored).unwrap(), b"%PDF-1.7 fake");
        let modified = fs::metadata(&stored).unwrap().modified().unwrap();

        // Storing again is a no-op and leaves no temporary files behind.
        assert_eq!(library.store(&source, &sha).await.unwrap(), stored);
        assert_eq!(fs::metadata(&stored).unwrap().modified().unwrap(), modified);
        let entries: Vec<_> = fs::read_dir(dir.join(LIBRARY_DIR)).unwrap().collect();
        assert_eq!(entries.len(), 1);
    }

    #[tokio::test]
    async fn removes_a_document_file() {
        let dir = temp_dir("remove");
        let source = dir.join("a.pdf");
        fs::write(&source, b"%PDF a").unwrap();
        let library = FsLibrary::new(dir.join(LIBRARY_DIR));
        let sha = library.digest(&source).await.unwrap().sha256;
        let stored = library.store(&source, &sha).await.unwrap();

        library.remove(&sha).await.unwrap();
        assert!(!stored.exists());
        assert!(source.exists(), "the user's file is never touched");
        library.remove(&sha).await.unwrap(); // missing is fine
    }

    #[tokio::test]
    async fn prunes_only_old_library_files_nobody_references() {
        let dir = temp_dir("prune");
        let library = FsLibrary::new(&dir);
        let (kept, orphan, tmp) = ("a".repeat(64), "b".repeat(64), "c".repeat(64));
        for name in [
            format!("{kept}.pdf"),
            format!("{orphan}.pdf"),
            format!("{tmp}.pdf.tmp-123"),
            "notes.txt".to_string(),
        ] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        // Just written: within the grace period, nothing goes.
        assert_eq!(library.prune(vec![kept.clone()]).await.unwrap(), 0);

        let keep = [kept.clone()].into();
        assert_eq!(prune(&dir, &keep, 0).unwrap(), 2);
        let mut left: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, [format!("{kept}.pdf"), "notes.txt".to_string()]);
        assert_eq!(library_hash(&format!("{}.pdf", "z".repeat(64))), None);
        let _ = (orphan, tmp);
    }

    #[tokio::test]
    async fn reports_missing_files() {
        let library = FsLibrary::new(temp_dir("missing"));
        let err = library
            .digest(Path::new("/nonexistent/file.pdf"))
            .await
            .unwrap_err();
        assert!(err.message.contains("/nonexistent/file.pdf"));
    }
}
