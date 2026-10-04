//! Documents of any format (ADR 0013): `save_processed` stores the metadata, the structure
//! (`document_sections`) and each chunk with its location, `chunks_of` and `sections_of` read
//! them back.
//!
//! A chunk keeps its page, page_end and bboxes in `document_chunks` for a PDF (the viewer and
//! the citations read them there) and a placeholder page 1 for any other format; its
//! `SourceLocation` and metadata are in `chunk_provenance`. A PDF chunk saved before
//! `chunk_provenance` existed has no row there: it is read as a `SourceLocation::Pdf` built from
//! its page columns.

use std::collections::HashMap;

use nlmx_application::ports::{SectionRecord, StoredExtraction};
use nlmx_domain::{
    document_type::DocumentType,
    ingestion::{DocumentId, SECTION_SEPARATOR},
    parsed::{ChunkMetadata, DocumentChunk, SectionKind},
    source::SourceLocation,
};
use rusqlite::{Connection, params};

use crate::{
    documents::{boxes_json, parse_type, update_job},
    lexical::parse_boxes,
};

/// What a save rejects because the data contradicts itself; nothing is written.
#[derive(Debug)]
struct Inconsistent(String);

impl std::fmt::Display for Inconsistent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Inconsistent {}

fn inconsistent(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(Inconsistent(message.into())))
}

fn json<T: serde::Serialize>(value: &T) -> rusqlite::Result<String> {
    serde_json::to_string(value).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

fn from_json<T: serde::de::DeserializeOwned>(text: &str) -> rusqlite::Result<T> {
    serde_json::from_str(text).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

/// Replaces everything derived from the file with `extraction`, in one transaction, so saving
/// the same result twice leaves the same rows. FTS rows, vec0 rows and `chunk_embeddings` follow
/// the deleted chunks through their triggers and cascades, exactly as in `save_extraction`.
pub fn save_processed(
    conn: &mut Connection,
    id: DocumentId,
    ex: &StoredExtraction,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let format: String = tx.query_row("SELECT format FROM documents WHERE id = ?1", [id], |r| {
        r.get(0)
    })?;
    let kind = parse_type(format)?;

    // Validate before writing anything.
    if !ex.pages.is_empty() && !kind.is_paged() {
        return Err(inconsistent(format!("um documento {kind} não tem páginas")));
    }
    for chunk in &ex.chunks {
        if chunk.location.document_type() != kind || chunk.metadata.document_type != kind {
            return Err(inconsistent(format!(
                "um trecho de outro formato não cabe em um documento {kind}"
            )));
        }
        if chunk.location.validate().is_err() {
            return Err(inconsistent("localização de trecho inválida"));
        }
    }

    tx.execute("DELETE FROM document_chunks WHERE document_id = ?1", [id])?;
    tx.execute("DELETE FROM document_sections WHERE document_id = ?1", [id])?;
    tx.execute("DELETE FROM document_pages WHERE document_id = ?1", [id])?;

    let is_pdf = kind == DocumentType::Pdf;
    let metadata = &ex.metadata;
    let changed = tx.execute(
        "UPDATE documents SET title = ?2, author = ?3, language = ?4, pdf_created_at = ?5,
             page_count = ?6, has_text_layer = ?7, metadata = ?8, extractor_version = ?9,
             normalizer_version = ?10, chunker_version = ?11, status = ?12, error = NULL
         WHERE id = ?1",
        params![
            id,
            ex.title,
            metadata.author,
            metadata.language,
            if is_pdf {
                metadata.created_at.as_deref()
            } else {
                None
            },
            is_pdf.then(|| metadata.page_count.unwrap_or(ex.pages.len() as u32)),
            is_pdf.then(|| ex.pages.iter().any(|p| p.has_text)),
            json(metadata)?,
            ex.extractor_version,
            ex.normalizer_version,
            ex.chunker_version,
            ex.status.as_str()
        ],
    )?;
    if changed == 0 {
        return Err(rusqlite::Error::QueryReturnedNoRows);
    }

    {
        let mut page = tx.prepare(
            "INSERT INTO document_pages (document_id, page_number, width, height, char_count, has_text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for p in &ex.pages {
            page.execute(params![
                id,
                p.number,
                p.width,
                p.height,
                p.char_count,
                p.has_text
            ])?;
        }
    }

    // Structure. The parent of a heading is the nearest earlier section whose path is its
    // path minus its own title; the parent of a body section is the heading it continues.
    let mut section_ids: Vec<(Vec<String>, i64)> = Vec::new();
    {
        let mut insert = tx.prepare(
            "INSERT INTO document_sections (document_id, parent_id, kind, title, level, ordinal, path, locator)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) RETURNING id",
        )?;
        for (ordinal, section) in ex.outline.iter().enumerate() {
            let parent_path: &[String] = match section.kind {
                SectionKind::Heading => section.path.split_last().map_or(&[], |(_, rest)| rest),
                SectionKind::Body => &section.path,
            };
            let parent = (!parent_path.is_empty())
                .then(|| {
                    section_ids
                        .iter()
                        .rev()
                        .find(|(path, _)| path == parent_path)
                        .map(|(_, id)| *id)
                })
                .flatten();
            let section_id: i64 = insert.query_row(
                params![
                    id,
                    parent,
                    section.kind.as_str(),
                    section.title,
                    section.level,
                    ordinal as u32,
                    json(&section.path)?,
                    json(&section.location)?
                ],
                |r| r.get(0),
            )?;
            section_ids.push((section.path.clone(), section_id));
        }
    }
    // A chunk belongs to the first section with its path (equal headings repeat).
    let mut by_path: HashMap<&[String], i64> = HashMap::new();
    for (path, section_id) in &section_ids {
        by_path.entry(path.as_slice()).or_insert(*section_id);
    }

    {
        let mut chunk = tx.prepare(
            "INSERT INTO document_chunks
                 (document_id, ordinal, text, token_count, page_start, page_end, section_path, bboxes, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        let mut provenance = tx.prepare(
            "INSERT INTO chunk_provenance (chunk_id, section_id, locator, metadata) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for c in &ex.chunks {
            let section =
                (!c.section_path.is_empty()).then(|| c.section_path.join(SECTION_SEPARATOR));
            // The legacy columns: the real page for a PDF, the placeholder 1 otherwise.
            let (page_start, page_end, boxes) = match &c.location {
                SourceLocation::Pdf {
                    page_start,
                    page_end,
                    boxes,
                } => (*page_start, *page_end, boxes_json(boxes)),
                _ => (1, 1, "[]".to_string()),
            };
            chunk.execute(params![
                id,
                c.index,
                c.text,
                c.token_count.unwrap_or(0),
                page_start,
                page_end,
                section,
                boxes,
                c.content_hash
            ])?;
            let chunk_id = tx.last_insert_rowid();
            // The boxes stay in `document_chunks.bboxes`; the locator keeps only the pages.
            let locator = match &c.location {
                SourceLocation::Pdf {
                    page_start,
                    page_end,
                    ..
                } => SourceLocation::Pdf {
                    page_start: *page_start,
                    page_end: *page_end,
                    boxes: Vec::new(),
                },
                other => other.clone(),
            };
            let section_id = by_path.get(c.section_path.as_slice()).copied();
            provenance.execute(params![
                chunk_id,
                section_id,
                json(&locator)?,
                json(&c.metadata)?
            ])?;
        }
    }
    update_job(&tx, id, ex.status, None)?;
    tx.commit()
}

/// The stored chunks of a document in order, with their location.
pub fn chunks_of(conn: &Connection, id: DocumentId) -> rusqlite::Result<Vec<DocumentChunk>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.ordinal, c.text, c.token_count, c.page_start, c.page_end, c.section_path,
                c.bboxes, c.content_hash, p.locator, p.metadata, s.path,
                d.title, d.original_filename, d.language
         FROM document_chunks c
         JOIN documents d ON d.id = c.document_id
         LEFT JOIN chunk_provenance p ON p.chunk_id = c.id
         LEFT JOIN document_sections s ON s.id = p.section_id
         WHERE c.document_id = ?1
         ORDER BY c.ordinal",
    )?;
    let rows = stmt.query_map([id], |r| {
        let boxes = parse_boxes(&r.get::<_, String>(7)?);
        let locator: Option<String> = r.get(9)?;
        let metadata: Option<String> = r.get(10)?;
        let section_path_json: Option<String> = r.get(11)?;
        let section_text: Option<String> = r.get(6)?;
        let location = match locator {
            // A PDF locator keeps only its pages; the boxes are in the legacy column.
            Some(text) => match from_json::<SourceLocation>(&text)? {
                SourceLocation::Pdf {
                    page_start,
                    page_end,
                    ..
                } => SourceLocation::Pdf {
                    page_start,
                    page_end,
                    boxes,
                },
                other => other,
            },
            None => SourceLocation::Pdf {
                page_start: r.get(4)?,
                page_end: r.get(5)?,
                boxes,
            },
        };
        let metadata = match metadata {
            Some(text) => from_json::<ChunkMetadata>(&text)?,
            None => ChunkMetadata {
                document_type: DocumentType::Pdf,
                document_title: r.get(12)?,
                file_name: r.get(13)?,
                language: r.get(14)?,
                columns: Vec::new(),
            },
        };
        // The exact path comes from the section (titles may contain the display separator).
        let section_path = match section_path_json {
            Some(text) => from_json::<Vec<String>>(&text)?,
            None => section_text
                .map(|text| text.split(SECTION_SEPARATOR).map(str::to_string).collect())
                .unwrap_or_default(),
        };
        Ok(DocumentChunk {
            document_id: id,
            chunk_id: Some(r.get(0)?),
            index: r.get(1)?,
            text: r.get(2)?,
            token_count: Some(r.get(3)?),
            section_path,
            location,
            metadata,
            content_hash: r.get(8)?,
        })
    })?;
    rows.collect()
}

/// The stored structure of a document in reading order.
pub fn sections_of(conn: &Connection, id: DocumentId) -> rusqlite::Result<Vec<SectionRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, parent_id, kind, title, level, ordinal, path, locator
         FROM document_sections WHERE document_id = ?1 ORDER BY ordinal",
    )?;
    let rows = stmt.query_map([id], |r| {
        let kind: String = r.get(2)?;
        Ok(SectionRecord {
            id: r.get(0)?,
            parent_id: r.get(1)?,
            kind: SectionKind::parse(&kind).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    format!("tipo de seção desconhecido: {kind}").into(),
                )
            })?,
            title: r.get(3)?,
            level: r.get(4)?,
            ordinal: r.get(5)?,
            path: from_json(&r.get::<_, String>(6)?)?,
            location: from_json(&r.get::<_, String>(7)?)?,
        })
    })?;
    rows.collect()
}
