//! Every stage of the indexing pipeline, for each of the five formats, on the real stack
//! (parsers, normalizer, chunker, SQLite with FTS5 and sqlite-vec): detection → parser →
//! normalization → structure → chunking → embeddings → persistence → retrieval → provenance.
//! The content is known, so each stage is checked against what it must have produced.

mod support;

use std::path::{Path, PathBuf};

use nlmx_application::ports::{
    ChunkReader, DocumentRepository, EmbeddingSource, LexicalIndex, VectorStore,
};
use nlmx_domain::{
    document_type::DocumentType,
    embedding::EmbeddingPurpose,
    ingestion::{ChunkPolicy, DocumentStatus},
    retrieval::{LexicalQuery, RetrievalFilter},
    vectors::{EmbeddingSpace, VectorFilter},
};
use nlmx_store_sqlite::Database;
use support::{
    fixture,
    multiformat::{App, a_word_of, epub_fixture, imported},
    root,
};
use unicode_normalization::is_nfc;

struct Case {
    file: &'static str,
    source: PathBuf,
    kind: DocumentType,
    /// Titles the structure must contain (headings, chapters).
    headings: &'static [&'static str],
    /// Words that must survive parsing, normalization and chunking.
    words: &'static [&'static str],
    /// What the label of every chunk's location must mention.
    label_has: &'static str,
}

fn corpus(name: &str) -> PathBuf {
    root().join("tests/golden/corpus").join(name)
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            file: "report.pdf",
            source: fixture("report.pdf"),
            kind: DocumentType::Pdf,
            headings: &["3. Prazos"],
            words: &["assinatura", "oitenta", "cobertura"],
            label_has: "p",
        },
        Case {
            file: "manual.md",
            source: corpus("manual.md"),
            kind: DocumentType::Markdown,
            headings: &["Manutenção", "Erros conhecidos", "Compras"],
            words: &["janela", "resfriamento", "Aurora", "E-4471"],
            label_has: "Manual de Operação",
        },
        Case {
            file: "reuniao.txt",
            source: corpus("reuniao.txt"),
            kind: DocumentType::Text,
            headings: &[],
            words: &["Orquídea", "nobreaks", "Ipê", "quarenta"],
            label_has: "caracteres",
        },
        Case {
            file: "vendas.csv",
            source: corpus("vendas.csv"),
            kind: DocumentType::Csv,
            headings: &[],
            words: &["Tapioca", "Dourada", "Granola", "Distribuidora"],
            label_has: "linha",
        },
        Case {
            file: "livro.epub",
            source: epub_fixture("livro.epub"),
            kind: DocumentType::Epub,
            headings: &["Partida", "Travessia", "Chegada"],
            words: &["inverno", "mar", "porto", "Santos"],
            label_has: "cap.",
        },
    ]
}

const INVISIBLE: [char; 6] = [
    '\u{feff}', '\u{200b}', '\u{200c}', '\u{200d}', '\u{00ad}', '\u{00a0}',
];

#[test]
fn detection_reads_the_format_from_the_extension_and_the_media_type() {
    for (name, kind) in [
        ("a.pdf", Some(DocumentType::Pdf)),
        ("A.PDF", Some(DocumentType::Pdf)),
        ("a.md", Some(DocumentType::Markdown)),
        ("a.markdown", Some(DocumentType::Markdown)),
        ("a.MD", Some(DocumentType::Markdown)),
        ("a.txt", Some(DocumentType::Text)),
        ("a.text", Some(DocumentType::Text)),
        ("a.csv", Some(DocumentType::Csv)),
        ("a.epub", Some(DocumentType::Epub)),
        ("a.docx", None),
        ("a.tar.gz", None),
        ("semextensao", None),
        (".md", None),
    ] {
        assert_eq!(DocumentType::from_path(Path::new(name)), kind, "{name}");
    }
    for (mime, kind) in [
        ("application/pdf", Some(DocumentType::Pdf)),
        ("text/markdown; charset=utf-8", Some(DocumentType::Markdown)),
        ("TEXT/PLAIN", Some(DocumentType::Text)),
        ("text/csv", Some(DocumentType::Csv)),
        ("application/epub+zip", Some(DocumentType::Epub)),
        ("application/zip", None),
    ] {
        assert_eq!(DocumentType::from_mime(mime), kind, "{mime}");
    }
    // Only a PDF has a preview; every format is indexable.
    assert!(
        DocumentType::ALL
            .iter()
            .filter(|k| k.previewable())
            .eq([&DocumentType::Pdf])
    );
}

#[tokio::test]
async fn every_stage_does_its_part_for_every_format() {
    for case in cases() {
        let Case { file, kind, .. } = case;
        let app = App::new(&format!("stages-{file}"), true);
        let path = app.user_file(&case.source, file);

        // ── Detection and parser: the right parser took it, as a document of its format.
        assert_eq!(DocumentType::from_path(&path), Some(kind), "{file}");
        let (id, chunk_count, status) = imported(app.ingestion.import(&path).await);
        assert_eq!(status, DocumentStatus::Indexed, "{file}");
        let record = app.db.get(id).await.unwrap().unwrap();
        assert_eq!(record.document_type, kind, "{file}");
        assert_eq!(record.mime_type, kind.mime_types()[0], "{file}");
        assert_eq!(record.previewable(), kind == DocumentType::Pdf, "{file}");

        // ── Chunking: contiguous, within the policy, located in the document's own format.
        let chunks = app.db.chunks_of(id).await.unwrap();
        assert_eq!(chunks.len() as u32, chunk_count, "{file}");
        assert!(!chunks.is_empty(), "{file}");
        let policy = ChunkPolicy::default();
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.index as usize, i, "{file}: ordinals are contiguous");
            assert_eq!(chunk.document_id, id, "{file}");
            assert!(!chunk.text.trim().is_empty(), "{file}: chunk {i} is empty");
            let tokens = chunk.token_count.expect("token count");
            assert!(
                tokens > 0 && tokens <= policy.max_tokens + policy.overlap_tokens,
                "{file}: {tokens}"
            );
            // ── Provenance: valid, of the document's format, and readable.
            chunk
                .location
                .validate()
                .unwrap_or_else(|e| panic!("{file}: {e}"));
            assert_eq!(chunk.location.document_type(), kind, "{file}");
            assert_eq!(chunk.metadata.document_type, kind, "{file}");
            assert_eq!(chunk.metadata.file_name.as_deref(), Some(file), "{file}");
            let label = chunk.location.label();
            assert!(
                label.contains(case.label_has)
                    || chunk
                        .section_path
                        .iter()
                        .any(|s| label.contains(s.as_str())),
                "{file}: {label}"
            );
            // ── Normalization: composed Unicode, no invisible characters.
            assert!(is_nfc(&chunk.text), "{file}: chunk {i} is not NFC");
            assert!(
                !chunk.text.contains(INVISIBLE),
                "{file}: chunk {i} has invisible characters"
            );
        }
        // Nothing was lost on the way: every word of the content is in some chunk.
        let all: String = chunks
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for word in case.words {
            assert!(all.contains(word), "{file}: lost {word:?}");
        }

        // ── Structure: the headings (or chapters) were found and stored.
        let sections = app.db.sections_of(id).await.unwrap();
        let titles: Vec<&str> = sections.iter().filter_map(|s| s.title.as_deref()).collect();
        for heading in case.headings {
            assert!(
                titles.contains(heading),
                "{file}: no section {heading:?} in {titles:?}"
            );
        }
        if case.headings.is_empty() {
            assert!(titles.is_empty(), "{file}: {titles:?}");
        }
        for section in &sections {
            assert_eq!(section.location.document_type(), kind, "{file}");
        }
        // A format with headings gives its chunks their section path.
        if matches!(kind, DocumentType::Markdown | DocumentType::Epub) {
            assert!(chunks.iter().any(|c| !c.section_path.is_empty()), "{file}");
        }

        // ── Persistence: what a second connection reads is what was saved, and the PDF keeps
        // its pages where the viewer reads them while the others have a provenance row each.
        let reopened = Database::open(app.dir.join("nlmx.sqlite3")).unwrap();
        assert_eq!(reopened.chunks_of(id).await.unwrap(), chunks, "{file}");
        let provenance = app.count(&format!(
            "SELECT count(*) FROM chunk_provenance p JOIN document_chunks c ON c.id = p.chunk_id WHERE c.document_id = {id}"
        ));
        assert_eq!(provenance as usize, chunks.len(), "{file}");
        if kind == DocumentType::Pdf {
            let pages = app.count(&format!(
                "SELECT count(*) FROM document_pages WHERE document_id = {id}"
            ));
            assert!(pages > 0, "{file}: the viewer needs the pages");
        } else {
            let pages = app.count(&format!(
                "SELECT count(*) FROM document_pages WHERE document_id = {id}"
            ));
            assert_eq!(pages, 0, "{file}: only a paged format has pages");
        }

        // ── Embeddings: one vector per chunk, in the model's space, and the vector of a chunk's
        // text finds that chunk first.
        let embedded = app.count(&format!(
            "SELECT count(*) FROM chunk_embeddings WHERE document_id = {id}"
        ));
        assert_eq!(embedded as usize, chunks.len(), "{file}");
        let provider = app.model.current().unwrap();
        let space = EmbeddingSpace::from_identity(&provider.identity().await.unwrap());
        let index = app.db.create_index(&space).await.unwrap();
        let views = app.db.document_chunks(id).await.unwrap();
        for view in &views {
            // The text as the embedding stage builds it: the section path first.
            let input = match &view.section {
                Some(section) => format!("{section}\n{}", view.text),
                None => view.text.clone(),
            };
            let vector = provider
                .embed(&input, EmbeddingPurpose::Passage)
                .await
                .unwrap();
            assert_eq!(vector.len() as u32, space.dimensions, "{file}");
            let nearest = VectorStore::search(
                app.db.as_ref(),
                index.id,
                &vector,
                1,
                &VectorFilter::default(),
            )
            .await
            .unwrap();
            assert_eq!(nearest[0].chunk_id, view.chunk_id, "{file}: KNN");
        }

        // ── Retrieval: the words of a chunk find it lexically (FTS5), with the same location.
        let target = &views[views.len() / 2];
        let word = a_word_of(&target.text);
        let hits = LexicalIndex::search(
            app.db.as_ref(),
            &LexicalQuery::from_query(&word),
            50,
            &RetrievalFilter::default(),
        )
        .await
        .unwrap();
        assert!(
            hits.iter().any(|h| h.chunk_id == target.chunk_id),
            "{file}: {word}"
        );
        assert_eq!(target.document_type, kind, "{file}");
        let stored = chunks
            .iter()
            .find(|c| c.chunk_id == Some(target.chunk_id))
            .unwrap();
        assert_eq!(
            target.location, stored.location,
            "{file}: the retrieval reads the same location"
        );
    }
}

/// Texts and tables that are hard on a parser end up readable, or are refused cleanly.
#[tokio::test]
async fn awkward_inputs_are_indexed_or_refused_with_a_reason() {
    let app = App::new("awkward", true);
    let inbox = app.dir.join("inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    let write = |name: &str, bytes: &[u8]| {
        let path = inbox.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    };

    // Accepted: a BOM and CRLF, a Latin-1 text, a semicolon CSV with a quoted line break,
    // Markdown with front matter and code, a UTF-16 text.
    let utf16: Vec<u8> = [0xfeffu16]
        .into_iter()
        .chain("Reunião com acentuação e ç.\n".encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect();
    let accepted = [
        write("bom.txt", "\u{feff}Primeira linha com acentuação.\r\nSegunda linha.\r\n".as_bytes()),
        write("latin1.txt", &[0x43, 0x61, 0x66, 0xe9, 0x20, 0x63, 0x6f, 0x6d, 0x20, 0x61, 0xe7, 0xfa, 0x63, 0x61, 0x72, 0x0a]),
        write("utf16.txt", &utf16),
        write("quebra.csv", "nome;nota\nAna;\"linha um\nlinha dois\"\nBia;9\n".as_bytes()),
        write("codigo.md", "---\ntitle: T\n---\n# Título\n\n```rust\nfn main() {}\n```\n\nTexto depois do código.\n".as_bytes()),
        write("setext.md", "Título setext\n=============\n\nCorpo do texto.\n".as_bytes()),
    ];
    for path in &accepted {
        let outcome = app.ingestion.import(path).await;
        let (id, chunks, status) = imported(outcome);
        assert!(chunks > 0, "{}", path.display());
        assert_eq!(status, DocumentStatus::Indexed, "{}", path.display());
        let text: String = app
            .db
            .chunks_of(id)
            .await
            .unwrap()
            .iter()
            .map(|c| c.text.clone())
            .collect();
        assert!(!text.contains(INVISIBLE), "{}", path.display());
    }
    // Each decoded to what a reader sees.
    let find = |needle: &str| {
        app.count(&format!(
            "SELECT count(*) FROM document_chunks WHERE text LIKE '%{needle}%'"
        ))
    };
    assert_eq!(find("Café com açúcar"), 1, "Latin-1");
    assert_eq!(find("acentuação e ç"), 1, "UTF-16");
    assert_eq!(
        find("linha um"),
        1,
        "a quoted line break stays in its record"
    );

    // Refused with a reason, and nothing indexed for them.
    let refused = [
        ("vazio.txt", Vec::new()),
        ("so-espacos.md", b"   \n\n  ".to_vec()),
        ("binario.txt", vec![0u8, 159, 146, 150, 0, 1, 2, 3]),
        ("falso.epub", b"isto nao e um zip".to_vec()),
        ("falso.pdf", b"%PDF-1.7 lixo".to_vec()),
    ];
    for (name, bytes) in refused {
        let path = write(name, &bytes);
        let outcome = app.ingestion.import(&path).await;
        let nlmx_domain::ingestion::ImportOutcome::Failed {
            id: Some(id),
            reason,
        } = outcome
        else {
            panic!("{name}: {outcome:?}")
        };
        assert!(!reason.is_empty(), "{name}");
        assert_eq!(app.status(id).await, DocumentStatus::Failed, "{name}");
        let chunks = app.count(&format!(
            "SELECT count(*) FROM document_chunks WHERE document_id = {id}"
        ));
        assert_eq!(chunks, 0, "{name}: nothing is indexed for a refused file");
    }
}
