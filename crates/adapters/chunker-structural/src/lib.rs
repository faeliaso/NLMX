//! Chunking: blocks → section-aware chunks with a token target and overlap, keeping the
//! source location (pages and boxes, lines, offsets, rows, chapters). Pure Rust,
//! deterministic.
//!
//! One algorithm (`core`) serves every format. [`StructuralChunker`] is the PDF pipeline's
//! [`Chunker`] (`StructuredDocument` → `ChunkDraft`); [`MultiFormatChunker`] is the
//! [`DocumentChunker`] over a `ParsedDocument` of any format; [`RecordChunker`] chunks the rows
//! of a table-like file.

mod core;
#[cfg(test)]
mod equivalence;
#[cfg(test)]
mod legacy;
#[cfg(test)]
mod multiformat_tests;
mod records;
mod split;

use nlmx_application::ports::{Chunker, DocumentChunker, TokenCounter};
use nlmx_domain::{
    document_type::DocumentType,
    ingestion::{Block, BlockKind, ChunkDraft, ChunkPolicy, PageBox, StructuredDocument},
    parsed::{
        ChunkContext, ContentBlock, ContentKind, DatasetColumn, DatasetMetadata, DocumentChunk,
        ParsedDocument,
    },
    source::SourceLocation,
};
use sha2::{Digest, Sha256};

pub use records::{RecordChunker, RecordChunks};

/// Bump when the PDF `Chunker`'s output for the same input changes (stored as
/// `documents.chunker_version`).
pub const VERSION: u32 = 1;

/// Approximate tokens for multilingual subword tokenizers: the larger of ~4/3 tokens per word and
/// ~1 token per 4 characters. Replaced by the embedding model's tokenizer once one is installed.
#[derive(Debug, Default, Clone, Copy)]
pub struct HeuristicTokenCounter;

impl TokenCounter for HeuristicTokenCounter {
    fn count(&self, text: &str) -> u32 {
        let words = text.split_whitespace().count() as u32;
        let chars = text.chars().count() as u32;
        (words * 4).div_ceil(3).max(chars.div_ceil(4))
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct StructuralChunker;

impl Chunker for StructuralChunker {
    fn version(&self) -> u32 {
        VERSION
    }

    fn chunk(
        &self,
        document: &StructuredDocument,
        policy: &ChunkPolicy,
        tokens: &dyn TokenCounter,
    ) -> Vec<ChunkDraft> {
        let blocks: Vec<ContentBlock> = document.blocks.iter().map(content_block).collect();
        let items: Vec<core::Item<'_>> = document
            .blocks
            .iter()
            .zip(&blocks)
            .map(|(block, content)| (block.section_path.as_slice(), content))
            .collect();
        core::chunk(&items, true, policy, tokens)
            .into_iter()
            .enumerate()
            .map(|(index, chunk)| draft(index as u32, chunk))
            .collect()
    }
}

/// A block of the PDF structure analysis as the core reads it. Its pages are the ones of its
/// boxes, or the page it starts on when it has none.
fn content_block(block: &Block) -> ContentBlock {
    let page_start = block
        .boxes
        .iter()
        .map(|b| b.page)
        .min()
        .unwrap_or(block.page);
    let page_end = block
        .boxes
        .iter()
        .map(|b| b.page)
        .max()
        .unwrap_or(block.page);
    ContentBlock {
        kind: match block.kind {
            BlockKind::Heading { level } => ContentKind::Heading { level },
            BlockKind::Paragraph => ContentKind::Paragraph,
            BlockKind::ListItem => ContentKind::ListItem {
                ordered: false,
                depth: 0,
            },
        },
        text: block.text.clone(),
        location: SourceLocation::Pdf {
            page_start,
            page_end,
            boxes: block.boxes.clone(),
        },
    }
}

fn draft(index: u32, chunk: core::CoreChunk) -> ChunkDraft {
    let (page_start, page_end, boxes): (u32, u32, Vec<PageBox>) = match chunk.location {
        SourceLocation::Pdf {
            page_start,
            page_end,
            boxes,
        } => (page_start, page_end, boxes),
        _ => (1, 1, Vec::new()),
    };
    ChunkDraft {
        index,
        token_count: chunk.token_count,
        page_start,
        page_end,
        section_path: chunk.section_path,
        boxes,
        content_hash: sha256_hex(&chunk.text),
        text: chunk.text,
    }
}

/// The [`DocumentChunker`] for every format: table-like files go through [`RecordChunker`],
/// the rest through the shared core. Chunks carry the document id and metadata of the context,
/// the section path (headings or chapters) and the merged source location of their blocks.
#[derive(Debug, Default, Clone, Copy)]
pub struct MultiFormatChunker;

impl MultiFormatChunker {
    /// Bump when the output for the same input changes.
    pub const VERSION: u32 = 1;
}

impl DocumentChunker for MultiFormatChunker {
    fn version(&self) -> u32 {
        Self::VERSION
    }

    fn chunk(
        &self,
        document: &ParsedDocument,
        context: &ChunkContext,
        policy: &ChunkPolicy,
        tokens: &dyn TokenCounter,
    ) -> Vec<DocumentChunk> {
        let kind = document.document_type();
        if kind == DocumentType::Csv {
            let dataset = document
                .metadata()
                .dataset
                .clone()
                .unwrap_or_else(|| dataset_of(document));
            return RecordChunker
                .chunk(
                    context,
                    &dataset,
                    document.blocks().cloned(),
                    policy,
                    tokens,
                )
                .collect();
        }
        let items: Vec<core::Item<'_>> = document
            .sections()
            .iter()
            .flat_map(|section| {
                section
                    .blocks
                    .iter()
                    .map(move |block| (section.path.as_slice(), block))
            })
            .collect();
        core::chunk(&items, kind.is_paged(), policy, tokens)
            .into_iter()
            .enumerate()
            .map(|(index, chunk)| DocumentChunk {
                document_id: context.document_id,
                chunk_id: None,
                index: index as u32,
                token_count: Some(chunk.token_count),
                section_path: chunk.section_path,
                location: chunk.location,
                metadata: context.metadata(kind, Vec::new()),
                content_hash: sha256_hex(&chunk.text),
                text: chunk.text,
            })
            .collect()
    }
}

/// The columns of the first record, for a table-like document without dataset metadata.
fn dataset_of(document: &ParsedDocument) -> DatasetMetadata {
    let columns = document
        .blocks()
        .find_map(|block| match &block.kind {
            ContentKind::Record { fields } => Some(
                fields
                    .iter()
                    .map(|f| DatasetColumn {
                        name: f.name.clone(),
                        kind: nlmx_domain::parsed::ColumnKind::Text,
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default();
    DatasetMetadata {
        columns,
        delimiter: ',',
        has_header: false,
        row_count: None,
    }
}

pub fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests;
