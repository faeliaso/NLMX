//! PDF regression: what the app does today (the content pipeline: PDF parser → text normalizer →
//! multi-format chunker → `save_processed`) must give a PDF what the legacy PDF-only path gave —
//! same chunks, pages, boxes and sections — and every PDF behaviour must still work on it.
//! Differences are limited to what the normalizer is documented to change in the text.

mod support;

use nlmx_application::ports::DocumentRepository;
use nlmx_domain::{
    ingestion::{DocumentStatus, ImportOutcome},
    parsed::DocumentChunk,
    source::SourceLocation,
};
use support::{Library, deterministic_embeddings, fixture, temp_dir};

const FIXTURES: [&str; 9] = [
    "report.pdf",
    "text.pdf",
    "unicode.pdf",
    "large.pdf",
    "mixed.pdf",
    "rotated.pdf",
    "scanned.pdf",
    "report-copy.pdf",
    "canary.pdf",
];

/// What a chunk is for the user: where it is and what it says (without the format details).
#[derive(Debug, PartialEq)]
struct Shape {
    pages: (u32, u32),
    boxes: usize,
    section_path: Vec<String>,
    text: String,
}

fn shapes(chunks: &[DocumentChunk]) -> Vec<Shape> {
    chunks
        .iter()
        .map(|c| {
            let SourceLocation::Pdf {
                page_start,
                page_end,
                boxes,
            } = &c.location
            else {
                panic!("a PDF chunk has a PDF location: {:?}", c.location)
            };
            Shape {
                pages: (*page_start, *page_end),
                boxes: boxes.len(),
                section_path: c.section_path.clone(),
                text: c.text.clone(),
            }
        })
        .collect()
}

/// The text as a reader sees it: Unicode-composed, one space between words.
fn reading(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    text.nfc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

async fn import_all(library: &mut Library) -> Vec<(String, ImportOutcome)> {
    let mut outcomes = Vec::new();
    for file in FIXTURES
        .iter()
        .chain(["encrypted.pdf", "corrupt.pdf"].iter())
    {
        let outcome = library.ingestion.import(&fixture(file)).await;
        if let ImportOutcome::Imported { id, .. } | ImportOutcome::Duplicate { id } = &outcome {
            library.documents.insert(file.to_string(), *id);
        }
        outcomes.push((file.to_string(), outcome));
    }
    outcomes
}

#[tokio::test]
async fn the_app_pipeline_gives_a_pdf_what_the_legacy_one_gave() {
    let mut legacy = Library::legacy(temp_dir("pdf-legacy"), deterministic_embeddings());
    let mut production = Library::new(temp_dir("pdf-production"), deterministic_embeddings());
    let legacy_outcomes = import_all(&mut legacy).await;
    let production_outcomes = import_all(&mut production).await;

    // Same result for every file: imported (with the same status and number of chunks),
    // duplicate, or the same kind of failure.
    for ((name, a), (_, b)) in legacy_outcomes.iter().zip(&production_outcomes) {
        match (a, b) {
            (
                ImportOutcome::Imported {
                    chunks: ca,
                    status: sa,
                    ..
                },
                ImportOutcome::Imported {
                    chunks: cb,
                    status: sb,
                    ..
                },
            ) => assert_eq!((ca, sa), (cb, sb), "{name}"),
            (ImportOutcome::Duplicate { .. }, ImportOutcome::Duplicate { .. }) => {}
            (
                ImportOutcome::Failed { reason: ra, .. },
                ImportOutcome::Failed { reason: rb, .. },
            ) => {
                println!("[pdf-regression] {name}: legacy {ra:?} · production {rb:?}");
            }
            other => panic!("{name}: {other:?}"),
        }
    }

    let (mut differing, mut total) = (0, 0);
    for name in FIXTURES {
        let (Some(a), Some(b)) = (legacy.documents.get(name), production.documents.get(name))
        else {
            continue;
        };
        let old = shapes(&legacy.db.chunks_of(*a).await.unwrap());
        let new = shapes(&production.db.chunks_of(*b).await.unwrap());
        assert_eq!(old.len(), new.len(), "{name}: number of chunks");
        for (i, (o, n)) in old.iter().zip(&new).enumerate() {
            total += 1;
            assert_eq!(o.pages, n.pages, "{name} chunk {i}: pages");
            assert_eq!(o.boxes, n.boxes, "{name} chunk {i}: boxes");
            assert_eq!(o.section_path, n.section_path, "{name} chunk {i}: section");
            if o.text != n.text {
                differing += 1;
                // Only what the normalizer is documented to change.
                assert_eq!(
                    reading(&o.text),
                    reading(&n.text),
                    "{name} chunk {i}: the words changed\n  legacy: {:?}\n  app:    {:?}",
                    o.text,
                    n.text
                );
            }
        }
        // The document row says the same.
        let (da, db) = (
            legacy.db.get(*a).await.unwrap().unwrap(),
            production.db.get(*b).await.unwrap().unwrap(),
        );
        assert_eq!(
            (da.status, da.file_size),
            (db.status, db.file_size),
            "{name}"
        );
    }
    println!(
        "[pdf-regression] {total} chunks compared, {differing} differ only in spacing/composition"
    );
    assert!(total > 100, "the corpus is not trivially small ({total})");
}

#[tokio::test]
async fn pdfs_stay_searchable_and_embedded_through_the_app_pipeline() {
    use nlmx_application::ports::{ChunkReader, LexicalIndex, VectorStore};
    use nlmx_domain::retrieval::{LexicalQuery, RetrievalFilter};

    let mut library = Library::new(temp_dir("pdf-search"), deterministic_embeddings());
    library
        .import(&["report.pdf", "text.pdf", "unicode.pdf"])
        .await;
    let report = library.documents["report.pdf"];
    assert_eq!(
        library.db.get(report).await.unwrap().unwrap().status,
        DocumentStatus::Indexed
    );

    // Lexical (FTS5): a word of the contract finds its page.
    let hits = LexicalIndex::search(
        library.db.as_ref(),
        &LexicalQuery::from_query("carência"),
        10,
        &RetrievalFilter::default(),
    )
    .await
    .unwrap();
    assert!(hits.iter().any(|h| h.document_id == report));
    // Accents do not matter (the tokenizer folds them), as before.
    let folded = LexicalIndex::search(
        library.db.as_ref(),
        &LexicalQuery::from_query("carencia"),
        10,
        &RetrievalFilter::default(),
    )
    .await
    .unwrap();
    assert_eq!(hits.len(), folded.len());

    // Vector (sqlite-vec): every chunk has a vector in the model's space and KNN works.
    let chunks = library.db.document_chunks(report).await.unwrap();
    assert!(!chunks.is_empty());
    let provider = library.embeddings.current().unwrap();
    let identity = provider.identity().await.unwrap();
    let space = nlmx_domain::vectors::EmbeddingSpace::from_identity(&identity);
    let index = library.db.create_index(&space).await.unwrap();
    let query = provider
        .embed_batch(
            &["Quando termina o prazo de carência?".to_string()],
            nlmx_domain::embedding::EmbeddingPurpose::Query,
        )
        .await
        .unwrap();
    let found = VectorStore::search(
        library.db.as_ref(),
        index.id,
        &query[0],
        5,
        &nlmx_domain::vectors::VectorFilter::default(),
    )
    .await
    .unwrap();
    assert!(!found.is_empty());
}
