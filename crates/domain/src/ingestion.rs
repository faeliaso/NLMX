//! Ingestion pipeline concepts: page layouts, document structure and chunks.

use std::fmt;

use crate::document::{BoundingBox, TextSpan};

pub type DocumentId = i64;

/// Processing status of an imported document (mirrors the `documents.status` CHECK constraint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentStatus {
    Queued,
    Extracting,
    Structuring,
    Chunking,
    /// Chunks are stored; waiting for (or computing) embeddings.
    Embedding,
    Indexed,
    /// No page has a text layer; needs OCR.
    NeedsOcr,
    Failed,
}

impl DocumentStatus {
    pub const ALL: [DocumentStatus; 8] = [
        Self::Queued,
        Self::Extracting,
        Self::Structuring,
        Self::Chunking,
        Self::Embedding,
        Self::Indexed,
        Self::NeedsOcr,
        Self::Failed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Extracting => "extracting",
            Self::Structuring => "structuring",
            Self::Chunking => "chunking",
            Self::Embedding => "embedding",
            Self::Indexed => "indexed",
            Self::NeedsOcr => "needs_ocr",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == value)
    }

    /// Statuses where the ingestion pipeline was interrupted and should be resumed.
    pub fn is_unfinished(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Extracting | Self::Structuring | Self::Chunking
        )
    }
}

impl fmt::Display for DocumentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One page as read from the document engine.
#[derive(Debug, Clone, PartialEq)]
pub struct PageLayout {
    pub number: u32,
    pub width: f32,
    pub height: f32,
    pub has_text: bool,
    pub char_count: u32,
    pub spans: Vec<TextSpan>,
}

/// A bounding box on a specific page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageBox {
    pub page: u32,
    pub bbox: BoundingBox,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// Level 1 is the most prominent heading.
    Heading {
        level: u8,
    },
    Paragraph,
    ListItem,
}

/// A normalized unit of content in reading order.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
    /// Page where the block starts.
    pub page: u32,
    pub boxes: Vec<PageBox>,
    /// Enclosing headings, outermost first. For a heading, its parents.
    pub section_path: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct StructuredDocument {
    pub blocks: Vec<Block>,
}

impl StructuredDocument {
    /// First top-level heading, used as a fallback title.
    pub fn first_heading(&self) -> Option<&str> {
        self.blocks
            .iter()
            .find(|b| matches!(b.kind, BlockKind::Heading { .. }))
            .map(|b| b.text.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkPolicy {
    pub target_tokens: u32,
    pub max_tokens: u32,
    pub overlap_tokens: u32,
    pub min_tokens: u32,
}

impl Default for ChunkPolicy {
    fn default() -> Self {
        Self {
            target_tokens: 350,
            max_tokens: 512,
            overlap_tokens: 50,
            min_tokens: 40,
        }
    }
}

/// A chunk ready to be stored (no embedding yet).
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkDraft {
    /// Position within the document, from 0.
    pub index: u32,
    pub text: String,
    /// Approximate token count.
    pub token_count: u32,
    /// First page of the chunk (the citation page).
    pub page_start: u32,
    pub page_end: u32,
    pub section_path: Vec<String>,
    pub boxes: Vec<PageBox>,
    /// SHA-256 (hex) of `text`.
    pub content_hash: String,
}

impl ChunkDraft {
    pub fn section(&self) -> Option<String> {
        (!self.section_path.is_empty()).then(|| self.section_path.join(" > "))
    }
}

/// Section path separator used for storage and display.
pub const SECTION_SEPARATOR: &str = " > ";

/// What happened to an imported file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportOutcome {
    Imported {
        id: DocumentId,
        chunks: u32,
        status: DocumentStatus,
    },
    Duplicate {
        id: DocumentId,
    },
    Failed {
        id: Option<DocumentId>,
        reason: String,
    },
}

/// Row shown in the document library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSummary {
    pub id: DocumentId,
    pub title: String,
    pub original_filename: String,
    pub page_count: Option<u32>,
    pub chunk_count: u32,
    pub status: DocumentStatus,
    pub error: Option<String>,
    pub imported_at: String,
}
