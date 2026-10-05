//! Contract of the document persistence for documents of any format.

use nlmx_application::ports::{DocumentRepository, InsertOutcome, NewDocument, StoredExtraction};
use nlmx_domain::{
    document_type::DocumentType,
    ingestion::{ChunkPolicy, DocumentStatus},
    parsed::{
        ChunkContext, ContentBlock, ContentKind, DocumentMetadata, DocumentSection, PageSummary,
        ParsedDocument,
    },
};

use crate::{FakeDocumentChunker, WordTokenCounter, sample_location_n, sample_structured_document};
use nlmx_application::ports::{DocumentChunker, PageRecord};

/// A document of `kind` with nested sections (a flat one for a CSV), each with paragraphs.
pub fn sample_nested_document(kind: DocumentType) -> ParsedDocument {
    if kind == DocumentType::Csv {
        return sample_structured_document(kind, 4);
    }
    let mut n = 0u32;
    let mut section = |path: &[&str], level: u8, paragraphs: u32| {
        let mut blocks = vec![ContentBlock {
            kind: ContentKind::Heading { level },
            text: path.last().unwrap().to_string(),
            location: sample_location_n(kind, {
                n += 1;
                n
            }),
        }];
        for _ in 0..paragraphs {
            blocks.push(ContentBlock {
                kind: ContentKind::Paragraph,
                text: format!("Parágrafo número {n} de {}.", path.last().unwrap()),
                location: sample_location_n(kind, {
                    n += 1;
                    n
                }),
            });
        }
        DocumentSection::new(
            Some(path.last().unwrap().to_string()),
            level,
            path.iter().map(|s| s.to_string()).collect(),
            blocks,
        )
        .unwrap()
    };
    let sections = vec![
        section(&["Cap 1"], 1, 2),
        section(&["Cap 1", "Sec 1.1"], 2, 2),
        section(&["Cap 1", "Sec 1.2"], 2, 1),
        section(&["Cap 2"], 1, 2),
    ];
    let pages = if kind.is_paged() {
        (1..=n + 1)
            .map(|number| PageSummary {
                number,
                width: 612.0,
                height: 792.0,
                char_count: 30,
                has_text: true,
            })
            .collect()
    } else {
        Vec::new()
    };
    ParsedDocument::new(
        kind,
        DocumentMetadata {
            title: Some("Título do documento".into()),
            author: Some("Autora".into()),
            language: Some("pt-BR".into()),
            ..Default::default()
        },
        pages,
        sections,
        Vec::new(),
    )
    .unwrap()
}

/// What `save_processed` takes for `document`, chunked by the fake chunker.
pub fn sample_stored_extraction(document: &ParsedDocument, document_id: i64) -> StoredExtraction {
    let context = ChunkContext {
        document_id,
        document_title: document.metadata().title.clone(),
        file_name: Some("arquivo.ext".into()),
        language: document.metadata().language.clone(),
    };
    let chunks = FakeDocumentChunker.chunk(
        document,
        &context,
        &ChunkPolicy::default(),
        &WordTokenCounter,
    );
    StoredExtraction {
        title: document
            .metadata()
            .title
            .clone()
            .unwrap_or_else(|| "sem título".into()),
        metadata: document.metadata().clone(),
        pages: document
            .pages()
            .iter()
            .map(|p| PageRecord {
                number: p.number,
                width: p.width,
                height: p.height,
                char_count: p.char_count,
                has_text: p.has_text,
            })
            .collect(),
        outline: document.outline(),
        chunks,
        extractor_version: 7,
        normalizer_version: 3,
        chunker_version: 5,
        status: DocumentStatus::Embedding,
    }
}

fn new_document(kind: DocumentType, sha: char) -> NewDocument {
    NewDocument {
        sha256: sha.to_string().repeat(64),
        original_filename: format!("arquivo.{}", kind.extensions()[0]),
        original_path: format!("/origem/arquivo.{}", kind.extensions()[0]),
        library_path: format!("/biblioteca/{sha}.{}", kind.extensions()[0]),
        file_size: 1234,
        document_type: kind,
        note_text: kind.is_note().then(|| "Texto da nota.".to_string()),
    }
}

/// The contract every `DocumentRepository` must honour for documents of every format.
pub async fn document_store_contract(repo: &dyn DocumentRepository) {
    for (kind, sha) in DocumentType::ALL
        .into_iter()
        .zip(['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'])
    {
        let new = new_document(kind, sha);
        let InsertOutcome::Inserted(id) = repo.insert(new.clone()).await.unwrap() else {
            panic!("{kind}: a new document is inserted");
        };

        // The format and its media type are kept; only a PDF is previewable.
        let record = repo.get(id).await.unwrap().expect("the document exists");
        assert_eq!(record.document_type, kind);
        assert_eq!(record.mime_type, kind.mime_types()[0]);
        assert_eq!(record.previewable(), kind == DocumentType::Pdf, "{kind}");
        assert_eq!(record.status, DocumentStatus::Queued);

        // The same content is the same document, whatever its name or format.
        let again = NewDocument {
            original_filename: "renomeado.txt".into(),
            document_type: DocumentType::Text,
            ..new
        };
        assert_eq!(
            repo.insert(again).await.unwrap(),
            InsertOutcome::AlreadyExists(id),
            "{kind}"
        );
        assert_eq!(
            repo.find_by_sha256(&sha.to_string().repeat(64))
                .await
                .unwrap(),
            Some(id)
        );
        assert_eq!(repo.get(id).await.unwrap().unwrap().document_type, kind);

        // Nothing is stored before a save.
        assert!(repo.chunks_of(id).await.unwrap().is_empty());
        assert!(repo.sections_of(id).await.unwrap().is_empty());

        // Save, then read back: chunks with their location and metadata, and the structure.
        let document = sample_nested_document(kind);
        let stored = sample_stored_extraction(&document, id);
        repo.save_processed(id, stored.clone()).await.unwrap();
        assert_eq!(
            repo.get(id).await.unwrap().unwrap().status,
            DocumentStatus::Embedding
        );

        let read = repo.chunks_of(id).await.unwrap();
        assert_eq!(read.len(), stored.chunks.len(), "{kind}");
        for (got, expected) in read.iter().zip(&stored.chunks) {
            assert!(got.chunk_id.is_some(), "a stored chunk has an id");
            let mut got = got.clone();
            got.chunk_id = None;
            assert_eq!(
                &got, expected,
                "{kind}: the chunk is read back as it was saved"
            );
        }

        let sections = repo.sections_of(id).await.unwrap();
        assert_eq!(sections.len(), stored.outline.len(), "{kind}");
        for (ordinal, (got, expected)) in sections.iter().zip(&stored.outline).enumerate() {
            assert_eq!(got.ordinal as usize, ordinal);
            assert_eq!(got.kind, expected.kind);
            assert_eq!(got.title, expected.title);
            assert_eq!(got.level, expected.level);
            assert_eq!(got.path, expected.path);
            assert_eq!(got.location, expected.location);
        }
        if kind != DocumentType::Csv {
            // "Sec 1.1" and "Sec 1.2" hang from "Cap 1"; both chapters are roots.
            let by_title = |title: &str| {
                sections
                    .iter()
                    .find(|s| s.title.as_deref() == Some(title))
                    .unwrap_or_else(|| panic!("{kind}: section {title}"))
            };
            assert_eq!(by_title("Cap 1").parent_id, None);
            assert_eq!(by_title("Cap 2").parent_id, None);
            assert_eq!(by_title("Sec 1.1").parent_id, Some(by_title("Cap 1").id));
            assert_eq!(by_title("Sec 1.2").parent_id, Some(by_title("Cap 1").id));
        }
        assert_eq!(repo.pages(id).await.unwrap().len(), stored.pages.len());

        // Saving again does not duplicate anything.
        repo.save_processed(id, stored.clone()).await.unwrap();
        let reread = repo.chunks_of(id).await.unwrap();
        assert_eq!(reread.len(), read.len(), "{kind}: no duplicate chunks");
        assert_eq!(repo.sections_of(id).await.unwrap().len(), sections.len());
        let documents = repo.list().await.unwrap();
        assert_eq!(documents.iter().filter(|d| d.id == id).count(), 1);
        assert_eq!(
            documents.iter().find(|d| d.id == id).unwrap().chunk_count as usize,
            stored.chunks.len()
        );

        // A chunk of another format is refused and nothing changes.
        let other = if kind == DocumentType::Csv {
            DocumentType::Markdown
        } else {
            DocumentType::Csv
        };
        let mut foreign = stored.clone();
        foreign.chunks = sample_stored_extraction(&sample_nested_document(other), id).chunks;
        assert!(repo.save_processed(id, foreign).await.is_err(), "{kind}");
        assert_eq!(
            repo.chunks_of(id).await.unwrap().len(),
            read.len(),
            "{kind}"
        );

        // Removing the document removes what it holds.
        let removed = repo.remove(id).await.unwrap().expect("removed");
        assert_eq!(
            removed.impact.chunks as usize,
            stored.chunks.len(),
            "{kind}"
        );
        assert!(repo.get(id).await.unwrap().is_none());
        assert!(repo.chunks_of(id).await.unwrap().is_empty());
        assert!(repo.sections_of(id).await.unwrap().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FakeDocumentRepository, llm_contract::block_on};

    #[test]
    fn the_fake_repository_honours_the_contract() {
        block_on(document_store_contract(&FakeDocumentRepository::default()));
    }

    #[test]
    fn sample_documents_chunk_and_validate_for_every_format() {
        for kind in DocumentType::ALL {
            let document = sample_nested_document(kind);
            assert_eq!(document.validate(), Ok(()), "{kind}");
            let stored = sample_stored_extraction(&document, 1);
            assert!(!stored.chunks.is_empty(), "{kind}");
            assert_eq!(stored.outline.len(), document.sections().len());
        }
    }
}
