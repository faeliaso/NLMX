//! Fakes and contracts of the pipeline stages after parsing: normalizer and chunker.

use std::{collections::HashSet, sync::Mutex};

use nlmx_application::ports::{DocumentChunker, DocumentNormalizer, TokenCounter};
use nlmx_domain::{
    document_type::DocumentType,
    ingestion::ChunkPolicy,
    parsed::{
        ChunkContext, ContentBlock, ContentKind, DocumentChunk, DocumentMetadata, DocumentSection,
        PageSummary, ParsedDocument,
    },
    source::SourceLocation,
};

/// A valid location of `kind` that differs for each `n` and grows with it.
pub fn sample_location_n(kind: DocumentType, n: u32) -> SourceLocation {
    match kind {
        DocumentType::Pdf => SourceLocation::pdf(n + 1, n + 1, vec![]),
        DocumentType::Markdown => {
            SourceLocation::markdown(vec!["Guia".into()], Some((n * 4 + 1, n * 4 + 3)))
        }
        DocumentType::Text => SourceLocation::text(n * 100, n * 100 + 80),
        DocumentType::Csv => SourceLocation::csv(n + 1, n + 1),
        DocumentType::Epub => SourceLocation::epub(1, Some("Capítulo 1".into()), None),
        DocumentType::Docx => SourceLocation::docx(vec!["Guia".into()], Some((n + 1, n + 1))),
        DocumentType::Xlsx => SourceLocation::xlsx(1, "Seção".into(), n + 1, n + 1),
        DocumentType::Note => SourceLocation::note(n * 100, n * 100 + 80),
    }
    .expect("a valid sample location")
}

/// A document of `kind` with one titled section ("Seção") of `paragraphs` paragraphs, each
/// "frase um. frase dois." plus its number. A CSV has records and no title; a PDF has pages.
pub fn sample_structured_document(kind: DocumentType, paragraphs: u32) -> ParsedDocument {
    let path = if kind == DocumentType::Csv {
        Vec::new()
    } else {
        vec!["Seção".to_string()]
    };
    let mut blocks = Vec::new();
    if kind != DocumentType::Csv {
        blocks.push(ContentBlock {
            kind: ContentKind::Heading { level: 1 },
            text: "Seção".into(),
            location: sample_location_n(kind, 0),
        });
    }
    for n in 0..paragraphs {
        let text = format!("Frase um. Frase dois do trecho número {n}.");
        blocks.push(ContentBlock {
            kind: ContentKind::Paragraph,
            text,
            location: sample_location_n(kind, n + 1),
        });
    }
    let pages = if kind.is_paged() {
        (1..=paragraphs + 1)
            .map(|number| PageSummary {
                number,
                width: 612.0,
                height: 792.0,
                char_count: 40,
                has_text: true,
            })
            .collect()
    } else {
        Vec::new()
    };
    let title = path.last().cloned();
    ParsedDocument::new(
        kind,
        DocumentMetadata::default(),
        pages,
        vec![
            DocumentSection::new(title, u8::from(!path.is_empty()), path, blocks)
                .expect("a section with blocks"),
        ],
        Vec::new(),
    )
    .expect("a valid sample document")
}

/// Normalizer that changes nothing; records how many documents it saw.
#[derive(Default)]
pub struct FakeDocumentNormalizer {
    seen: Mutex<u32>,
}

impl FakeDocumentNormalizer {
    pub fn documents_seen(&self) -> u32 {
        *self.seen.lock().unwrap()
    }
}

impl DocumentNormalizer for FakeDocumentNormalizer {
    fn version(&self) -> u32 {
        95
    }

    fn normalize(&self, document: ParsedDocument) -> ParsedDocument {
        *self.seen.lock().unwrap() += 1;
        document
    }
}

/// Chunker that makes one chunk of each non-heading block, keeping its location and path.
#[derive(Default)]
pub struct FakeDocumentChunker;

impl DocumentChunker for FakeDocumentChunker {
    fn version(&self) -> u32 {
        94
    }

    fn chunk(
        &self,
        document: &ParsedDocument,
        context: &ChunkContext,
        _policy: &ChunkPolicy,
        tokens: &dyn TokenCounter,
    ) -> Vec<DocumentChunk> {
        let mut chunks = Vec::new();
        for section in document.sections() {
            for block in &section.blocks {
                if matches!(block.kind, ContentKind::Heading { .. }) || block.text.trim().is_empty()
                {
                    continue;
                }
                chunks.push(DocumentChunk {
                    document_id: context.document_id,
                    chunk_id: None,
                    index: chunks.len() as u32,
                    token_count: Some(tokens.count(&block.text)),
                    text: block.text.clone(),
                    section_path: section.path.clone(),
                    location: block.location.clone(),
                    metadata: context.metadata(document.document_type(), Vec::new()),
                    // Not a real hash: the fake has no digest.
                    content_hash: format!("{:0>64x}", chunks.len()),
                });
            }
        }
        chunks
    }
}

fn kind_of(block: &ContentBlock) -> std::mem::Discriminant<ContentKind> {
    std::mem::discriminant(&block.kind)
}

/// The contract every `DocumentNormalizer` must honour, over `samples` that exercise it.
pub fn document_normalizer_contract(
    normalizer: &dyn DocumentNormalizer,
    samples: &[ParsedDocument],
) {
    assert!(!samples.is_empty(), "the contract needs samples");
    for sample in samples {
        let out = normalizer.normalize(sample.clone());
        assert_eq!(out.validate(), Ok(()), "the result is a valid document");
        assert_eq!(out.document_type(), sample.document_type());
        assert_eq!(out.pages(), sample.pages(), "pages are untouched");
        assert_eq!(out.warnings(), sample.warnings(), "warnings are untouched");

        // Only text changes: the blocks left are the input's, in order, with the same kind
        // and location (blocks that became empty may be gone).
        let mut input = sample.blocks();
        for block in out.blocks() {
            assert!(
                input.any(|b| kind_of(b) == kind_of(block) && b.location == block.location),
                "a block was invented or moved: {:?}",
                block.location
            );
        }
        if sample.has_text() {
            assert!(out.has_text(), "a document with text keeps its text");
        }

        // Idempotent.
        assert_eq!(
            normalizer.normalize(out.clone()),
            out,
            "normalizing twice changes nothing"
        );
    }
}

/// The contract every `DocumentChunker` must honour for one parsed document.
pub fn document_chunker_contract(
    chunker: &dyn DocumentChunker,
    document: &ParsedDocument,
    context: &ChunkContext,
    policy: &ChunkPolicy,
    tokens: &dyn TokenCounter,
) {
    let kind = document.document_type();
    let chunks = chunker.chunk(document, context, policy, tokens);
    assert_eq!(
        chunks,
        chunker.chunk(document, context, policy, tokens),
        "chunking is deterministic"
    );

    let body: Vec<(&[String], &ContentBlock)> = document
        .sections()
        .iter()
        .flat_map(|s| s.blocks.iter().map(move |b| (s.path.as_slice(), b)))
        .filter(|(_, b)| {
            !matches!(b.kind, ContentKind::Heading { .. }) && !b.text.trim().is_empty()
        })
        .collect();
    if body.is_empty() {
        assert!(chunks.is_empty(), "no body text, no chunks");
        return;
    }
    assert!(!chunks.is_empty(), "a document with text gives chunks");

    let paths: HashSet<&[String]> = document
        .sections()
        .iter()
        .map(|s| s.path.as_slice())
        .collect();
    for (position, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.index as usize, position, "indexes run 0..n");
        assert_eq!(chunk.document_id, context.document_id);
        assert_eq!(chunk.chunk_id, None, "ids exist only once stored");
        assert!(chunk.token_count.is_some_and(|n| n > 0), "token count");
        assert!(!chunk.text.trim().is_empty());
        assert_eq!(chunk.location.validate(), Ok(()), "{:?}", chunk.location);
        assert_eq!(chunk.location.document_type(), kind);
        assert_eq!(chunk.metadata.document_type, kind);
        assert_eq!(chunk.metadata.file_name, context.file_name);
        assert_eq!(chunk.metadata.document_title, context.document_title);
        assert_eq!(chunk.content_hash.len(), 64);
        assert!(
            paths.contains(chunk.section_path.as_slice()),
            "a chunk keeps the path of a section of the document: {:?}",
            chunk.section_path
        );
    }

    // Provenance: each body block's location is covered by a chunk of its section...
    for (path, block) in &body {
        assert!(
            chunks.iter().any(|chunk| {
                chunk.section_path.as_slice() == *path
                    && chunk.location.merge(&block.location).as_ref() == Some(&chunk.location)
            }),
            "no chunk covers the block at {:?}",
            block.location
        );
    }
    // ... and nothing the block said is lost.
    let words: HashSet<&str> = chunks
        .iter()
        .flat_map(|c| c.text.split_whitespace())
        .collect();
    for (_, block) in &body {
        for word in block.text.split_whitespace() {
            assert!(words.contains(word), "the word {word:?} is gone");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WordTokenCounter;

    fn context() -> ChunkContext {
        ChunkContext {
            document_id: 9,
            document_title: Some("Título".into()),
            file_name: Some("arquivo.ext".into()),
            language: None,
        }
    }

    #[test]
    fn sample_documents_are_valid_for_every_format() {
        for kind in DocumentType::ALL {
            let document = sample_structured_document(kind, 3);
            assert_eq!(document.validate(), Ok(()), "{kind}");
            assert!(document.has_text());
        }
    }

    #[test]
    fn the_fake_normalizer_honours_its_contract() {
        let samples: Vec<_> = DocumentType::ALL
            .into_iter()
            .map(|kind| sample_structured_document(kind, 2))
            .collect();
        let normalizer = FakeDocumentNormalizer::default();
        document_normalizer_contract(&normalizer, &samples);
        assert!(normalizer.documents_seen() >= samples.len() as u32);
    }

    #[test]
    fn the_fake_chunker_honours_its_contract() {
        for kind in DocumentType::ALL {
            let document = sample_structured_document(kind, 4);
            document_chunker_contract(
                &FakeDocumentChunker,
                &document,
                &context(),
                &ChunkPolicy::default(),
                &WordTokenCounter,
            );
        }
    }
}
