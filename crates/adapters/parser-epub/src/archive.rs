//! The EPUB container: a zip read with limits, and the paths inside it.

use std::io::{Cursor, Read};

use nlmx_domain::{document_type::DocumentType, parsed::ParseError};
use zip::{ZipArchive, result::ZipError};

/// Resource limits against zip bombs and absurd books.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Entries in the zip.
    pub max_entries: usize,
    /// Uncompressed bytes of one entry.
    pub max_entry_bytes: u64,
    /// Uncompressed bytes of everything that is read, and the size of the file itself.
    pub max_total_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_entry_bytes: 32 * 1024 * 1024,
            max_total_bytes: 256 * 1024 * 1024,
        }
    }
}

/// Why an entry could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadFailure {
    TooLarge,
    /// The entry is damaged or cannot be decoded.
    Corrupt,
}

pub(crate) struct Container<'a> {
    archive: ZipArchive<Cursor<&'a [u8]>>,
    limits: Limits,
    total: u64,
}

impl<'a> Container<'a> {
    pub(crate) fn open(bytes: &'a [u8], limits: Limits) -> Result<Self, ParseError> {
        if bytes.len() as u64 > limits.max_total_bytes {
            return Err(ParseError::TooLarge);
        }
        let archive = ZipArchive::new(Cursor::new(bytes))
            .map_err(|_| ParseError::Invalid(DocumentType::Epub))?;
        if archive.len() > limits.max_entries {
            return Err(ParseError::TooLarge);
        }
        Ok(Self {
            archive,
            limits,
            total: 0,
        })
    }

    /// The entry's bytes; `None` when there is no such entry.
    pub(crate) fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>, ReadFailure> {
        let entry = match self.archive.by_name(name) {
            Ok(entry) => entry,
            Err(ZipError::FileNotFound) => return Ok(None),
            Err(_) => return Err(ReadFailure::Corrupt),
        };
        let limit = self.limits.max_entry_bytes;
        if entry.size() > limit {
            return Err(ReadFailure::TooLarge);
        }
        let mut bytes = Vec::new();
        entry
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ReadFailure::Corrupt)?;
        if bytes.len() as u64 > limit {
            return Err(ReadFailure::TooLarge);
        }
        self.total += bytes.len() as u64;
        if self.total > self.limits.max_total_bytes {
            return Err(ReadFailure::TooLarge);
        }
        Ok(Some(bytes))
    }
}

/// The directory of a zip path (`OEBPS/content.opf` → `OEBPS`; no directory → ``).
pub(crate) fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The zip path an `href` points to, relative to `base_dir`: fragment and query dropped,
/// percent-escapes decoded, `.` and `..` resolved.
pub(crate) fn resolve(base_dir: &str, href: &str) -> String {
    let href = href.split(['#', '?']).next().unwrap_or_default();
    let href = percent_decode(href);
    let joined = match href.strip_prefix('/') {
        Some(absolute) => absolute.to_string(),
        None if base_dir.is_empty() => href,
        None => format!("{base_dir}/{href}"),
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(high * 16 + low);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|d| d as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hrefs_resolve_against_the_base_directory() {
        assert_eq!(resolve("OEBPS", "text/ch1.xhtml"), "OEBPS/text/ch1.xhtml");
        assert_eq!(resolve("OEBPS/text", "../img/a.png"), "OEBPS/img/a.png");
        assert_eq!(resolve("", "ch1.xhtml#sec"), "ch1.xhtml");
        assert_eq!(resolve("OEBPS", "./ch1.xhtml?x=1"), "OEBPS/ch1.xhtml");
        assert_eq!(resolve("OEBPS", "/ch1.xhtml"), "ch1.xhtml");
        assert_eq!(resolve("a", "../../x"), "x");
    }

    #[test]
    fn percent_escapes_are_decoded() {
        assert_eq!(
            resolve("OEBPS", "cap%C3%ADtulo%201.xhtml"),
            "OEBPS/capítulo 1.xhtml"
        );
        assert_eq!(resolve("", "100%.xhtml"), "100%.xhtml");
        assert_eq!(resolve("", "a%2"), "a%2");
    }

    #[test]
    fn directories_are_taken_from_paths() {
        assert_eq!(dir_of("OEBPS/content.opf"), "OEBPS");
        assert_eq!(dir_of("a/b/c.xhtml"), "a/b");
        assert_eq!(dir_of("content.opf"), "");
    }
}
