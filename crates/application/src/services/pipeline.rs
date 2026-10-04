//! The content pipeline, in explicit stages: **parse → normalize → chunk**. Embeddings are the
//! next stage and are not part of it: `EmbedDocuments` reads the chunks once they are stored.
//!
//! Each stage sits behind its own port and knows nothing about the others: the parser
//! extracts and structures, the normalizer cleans text, the chunker cuts chunks that keep the
//! section path and the source location.

use std::sync::Arc;

use nlmx_domain::{
    document_type::DocumentType,
    ingestion::{ChunkPolicy, DocumentId},
    parsed::{
        ChunkContext, DocumentChunk, DocumentMetadata, PageSummary, ParseError, ParseWarning,
        SectionOutline,
    },
};

use super::parsing::ParserRegistry;
use crate::ports::{BoxFuture, DocumentChunker, DocumentNormalizer, DocumentSource, TokenCounter};

/// Versions of the stages that made a result (so a stored result can be told stale).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageVersions {
    pub parser: u32,
    pub normalizer: u32,
    pub chunker: u32,
}

/// What the pipeline produces for one document.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedDocument {
    pub document_type: DocumentType,
    pub metadata: DocumentMetadata,
    /// Pages of a paged format (empty otherwise).
    pub pages: Vec<PageSummary>,
    /// The structure: its sections without their text.
    pub outline: Vec<SectionOutline>,
    /// A paged document none of whose pages has a text layer: it has no chunks.
    pub needs_ocr: bool,
    pub chunks: Vec<DocumentChunk>,
    pub warnings: Vec<ParseWarning>,
    pub versions: StageVersions,
}

/// A stage the pipeline is about to start (reported by [`ContentPipeline::run_with`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineStage {
    Parsing,
    /// Normalization (the parser already structured the document).
    Structuring,
    Chunking,
}

/// Told before each stage starts (the use case records the status and reports progress).
pub trait StageObserver: Send + Sync {
    fn started(&self, stage: PipelineStage) -> BoxFuture<'_, ()>;
}

struct Silent;

impl StageObserver for Silent {
    fn started(&self, _stage: PipelineStage) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

pub struct ContentPipeline {
    pub parsers: ParserRegistry,
    pub normalizer: Arc<dyn DocumentNormalizer>,
    pub chunker: Arc<dyn DocumentChunker>,
    pub tokens: Arc<dyn TokenCounter>,
    pub policy: ChunkPolicy,
}

impl ContentPipeline {
    /// Runs the three stages on a file. `file_name` is the original file's name, kept in the
    /// metadata of every chunk. A format that is not paged and has no text once normalized is
    /// `Empty`.
    pub async fn run(
        &self,
        source: &DocumentSource,
        document_id: DocumentId,
        file_name: Option<&str>,
    ) -> Result<ProcessedDocument, ParseError> {
        self.run_with(source, document_id, file_name, &Silent).await
    }

    /// Like [`run`](Self::run), telling `observer` before each stage starts.
    pub async fn run_with(
        &self,
        source: &DocumentSource,
        document_id: DocumentId,
        file_name: Option<&str>,
        observer: &dyn StageObserver,
    ) -> Result<ProcessedDocument, ParseError> {
        let (parser, resolved) = self.parsers.resolve(source)?;
        observer.started(PipelineStage::Parsing).await;
        let parsed = parser.parse(&resolved).await?;
        observer.started(PipelineStage::Structuring).await;
        let parsed = self.normalizer.normalize(parsed);

        let document_type = parsed.document_type();
        let needs_ocr = parsed.needs_ocr();
        if !parsed.has_text() && !document_type.is_paged() {
            return Err(ParseError::Empty);
        }
        let context = ChunkContext {
            document_id,
            document_title: parsed.metadata().title.clone(),
            file_name: file_name.map(str::to_string),
            language: parsed.metadata().language.clone(),
        };
        observer.started(PipelineStage::Chunking).await;
        let chunks = if needs_ocr {
            Vec::new()
        } else {
            self.chunker
                .chunk(&parsed, &context, &self.policy, self.tokens.as_ref())
        };
        let outline = parsed.outline();
        let (_, metadata, pages, _, warnings) = parsed.into_parts();
        Ok(ProcessedDocument {
            document_type,
            metadata,
            pages,
            outline,
            needs_ocr,
            chunks,
            warnings,
            versions: StageVersions {
                parser: parser.version(),
                normalizer: self.normalizer.version(),
                chunker: self.chunker.version(),
            },
        })
    }
}
