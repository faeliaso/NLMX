//! The OOXML package: a zip read with limits, plus the paths and relationships inside it.

use std::io::{Cursor, Read};

use nlmx_domain::{document_type::DocumentType, parsed::ParseError};
use zip::{ZipArchive, result::ZipError};

use crate::xml::{attr, lower, reader};

/// Signature of the legacy compound file that wraps a password-protected OOXML file.
const COMPOUND_FILE: [u8; 4] = [0xD0, 0xCF, 0x11, 0xE0];

/// Resource limits against zip bombs and absurd files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Entries in the zip.
    pub max_entries: usize,
    /// Uncompressed bytes of one entry (a worksheet is one entry).
    pub max_entry_bytes: u64,
    /// Uncompressed bytes of everything that is read, and the size of the file itself.
    pub max_total_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_entry_bytes: 96 * 1024 * 1024,
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

pub(crate) struct Package<'a> {
    archive: ZipArchive<Cursor<&'a [u8]>>,
    limits: Limits,
    total: u64,
    kind: DocumentType,
}

impl<'a> Package<'a> {
    pub(crate) fn open(
        bytes: &'a [u8],
        limits: Limits,
        kind: DocumentType,
    ) -> Result<Self, ParseError> {
        if bytes.len() as u64 > limits.max_total_bytes {
            return Err(ParseError::TooLarge);
        }
        if bytes.starts_with(&COMPOUND_FILE) {
            // A password-protected file, or a pre-2007 .doc/.xls: neither can be read.
            return Err(ParseError::Encrypted);
        }
        let archive = ZipArchive::new(Cursor::new(bytes)).map_err(|_| ParseError::Invalid(kind))?;
        if archive.len() > limits.max_entries {
            return Err(ParseError::TooLarge);
        }
        Ok(Self {
            archive,
            limits,
            total: 0,
            kind,
        })
    }

    pub(crate) fn invalid(&self) -> ParseError {
        ParseError::Invalid(self.kind)
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

    /// An entry the file cannot do without.
    pub(crate) fn required(&mut self, name: &str) -> Result<Vec<u8>, ParseError> {
        match self.read(name) {
            Ok(Some(bytes)) => Ok(bytes),
            Ok(None) | Err(ReadFailure::Corrupt) => Err(self.invalid()),
            Err(ReadFailure::TooLarge) => Err(ParseError::TooLarge),
        }
    }

    /// An entry that only adds detail: a missing or damaged one is `None`, but one over the
    /// limits still fails the file.
    pub(crate) fn optional(&mut self, name: &str) -> Result<Option<Vec<u8>>, ParseError> {
        match self.read(name) {
            Ok(found) => Ok(found),
            Err(ReadFailure::Corrupt) => Ok(None),
            Err(ReadFailure::TooLarge) => Err(ParseError::TooLarge),
        }
    }

    /// The path of the main part (`word/document.xml`, `xl/workbook.xml`) according to the
    /// package relationships, or `default` when they do not say.
    pub(crate) fn main_part(&mut self, default: &str) -> Result<String, ParseError> {
        let Some(bytes) = self.optional("_rels/.rels")? else {
            return Ok(default.to_string());
        };
        let found = relationships(&bytes)
            .into_iter()
            .find(|r| r.kind.ends_with("/officeDocument"))
            .map(|r| resolve("", &r.target));
        Ok(found.unwrap_or_else(|| default.to_string()))
    }
}

/// One `<Relationship>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Relationship {
    pub id: String,
    pub kind: String,
    pub target: String,
}

pub(crate) fn relationships(bytes: &[u8]) -> Vec<Relationship> {
    use quick_xml::events::Event;

    let mut reader = reader(bytes);
    let mut found = Vec::new();
    while let Ok(event) = reader.read_event() {
        match event {
            Event::Start(e) if lower(e.local_name().as_ref()) == "relationship" => {
                if let (Some(id), Some(kind), Some(target)) =
                    (attr(&e, "id"), attr(&e, "type"), attr(&e, "target"))
                {
                    let external = attr(&e, "targetmode").is_some_and(|m| m == "External");
                    if !external {
                        found.push(Relationship { id, kind, target });
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    found
}

/// The directory of a zip path (`word/document.xml` → `word`; no directory → ``).
pub(crate) fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The path of the relationships part of `part` (`xl/workbook.xml` → `xl/_rels/workbook.xml.rels`).
pub(crate) fn rels_of(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

/// The zip path a relationship `target` points to, relative to `base_dir`: `.` and `..`
/// resolved, a leading `/` meaning the package root.
pub(crate) fn resolve(base_dir: &str, target: &str) -> String {
    let joined = match target.strip_prefix('/') {
        Some(absolute) => absolute.to_string(),
        None if base_dir.is_empty() => target.to_string(),
        None => format!("{base_dir}/{target}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_resolve_against_the_base_directory() {
        assert_eq!(
            resolve("xl", "worksheets/sheet1.xml"),
            "xl/worksheets/sheet1.xml"
        );
        assert_eq!(
            resolve("xl/worksheets", "../sharedStrings.xml"),
            "xl/sharedStrings.xml"
        );
        assert_eq!(resolve("xl", "/xl/styles.xml"), "xl/styles.xml");
        assert_eq!(resolve("", "word/document.xml"), "word/document.xml");
    }

    #[test]
    fn relationship_parts_are_named_after_their_part() {
        assert_eq!(rels_of("xl/workbook.xml"), "xl/_rels/workbook.xml.rels");
        assert_eq!(rels_of("workbook.xml"), "_rels/workbook.xml.rels");
        assert_eq!(dir_of("xl/workbook.xml"), "xl");
    }
}
