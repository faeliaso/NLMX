//! `MultiFormatChunker` on documents of every format built by hand: the section path and the
//! source location of what a chunk holds must survive chunking.

use nlmx_application::ports::{DocumentChunker, TokenCounter};
use nlmx_domain::{
    document::BoundingBox,
    document_type::DocumentType,
    ingestion::{ChunkPolicy, PageBox},
    parsed::{
        ChunkContext, ColumnKind, ContentBlock, ContentKind, DatasetColumn, DatasetMetadata,
        DocumentChunk, DocumentMetadata, DocumentSection, ParsedDocument, RecordField,
    },
    source::SourceLocation,
};

use crate::{MultiFormatChunker, sha256_hex};

/// One token per word keeps the arithmetic obvious.
struct WordCounter;
impl TokenCounter for WordCounter {
    fn count(&self, text: &str) -> u32 {
        text.split_whitespace().count() as u32
    }
}

fn policy() -> ChunkPolicy {
    ChunkPolicy {
        target_tokens: 20,
        max_tokens: 30,
        overlap_tokens: 4,
        min_tokens: 5,
    }
}

fn context() -> ChunkContext {
    ChunkContext {
        document_id: 42,
        document_title: Some("Guia".into()),
        file_name: Some("guia.md".into()),
        language: Some("pt-BR".into()),
    }
}

fn block(kind: ContentKind, text: impl Into<String>, location: SourceLocation) -> ContentBlock {
    ContentBlock {
        kind,
        text: text.into(),
        location,
    }
}

fn section(path: &[&str], blocks: Vec<ContentBlock>) -> DocumentSection {
    let path: Vec<String> = path.iter().map(|s| s.to_string()).collect();
    DocumentSection::new(path.last().cloned(), path.len() as u8, path, blocks).unwrap()
}

fn document(kind: DocumentType, sections: Vec<DocumentSection>) -> ParsedDocument {
    ParsedDocument::new(kind, DocumentMetadata::default(), vec![], sections, vec![]).unwrap()
}

fn run(document: &ParsedDocument) -> Vec<DocumentChunk> {
    MultiFormatChunker.chunk(document, &context(), &policy(), &WordCounter)
}

/// `n` sentences of five words; the first word is `marker`, which identifies the block.
fn prose(marker: &str, n: usize) -> String {
    (0..n)
        .map(|i| {
            if i == 0 {
                format!("{marker} frase número {i} aqui.")
            } else {
                format!("Outra frase número {i} aqui.")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The provenance rules every chunk of every format must obey.
fn assert_provenance(document: &ParsedDocument, chunks: &[DocumentChunk]) {
    assert!(!chunks.is_empty());
    let paths: Vec<&[String]> = document
        .sections()
        .iter()
        .map(|s| s.path.as_slice())
        .collect();
    for (index, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.index, index as u32, "sequential positions");
        assert_eq!(chunk.document_id, 42);
        assert_eq!(chunk.chunk_id, None);
        assert_eq!(chunk.token_count, Some(WordCounter.count(&chunk.text)));
        assert_eq!(chunk.content_hash, sha256_hex(&chunk.text));
        assert_eq!(chunk.metadata.document_type, document.document_type());
        assert_eq!(chunk.metadata.file_name.as_deref(), Some("guia.md"));
        assert_eq!(chunk.metadata.document_title.as_deref(), Some("Guia"));
        assert_eq!(chunk.metadata.language.as_deref(), Some("pt-BR"));
        assert_eq!(chunk.location.validate(), Ok(()), "{:?}", chunk.location);
        assert_eq!(chunk.location.document_type(), document.document_type());
        assert!(
            paths.contains(&chunk.section_path.as_slice()),
            "{:?} is not a section path",
            chunk.section_path
        );
    }
    // A chunk's location covers the location of every block whose marker it contains, and every
    // non-heading block with a marker is in some chunk.
    for source in document.blocks() {
        if matches!(source.kind, ContentKind::Heading { .. }) {
            continue;
        }
        let Some(marker) = source.text.split_whitespace().next() else {
            continue;
        };
        let holders: Vec<_> = chunks
            .iter()
            .filter(|c| c.text.split_whitespace().any(|w| w == marker))
            .collect();
        assert!(!holders.is_empty(), "{marker} was lost");
        for chunk in holders {
            assert_eq!(
                chunk.location.merge(&source.location).as_ref(),
                Some(&chunk.location),
                "{marker}: {:?} does not cover {:?}",
                chunk.location,
                source.location
            );
        }
    }
}

fn md(path: &[&str], start: u32, end: u32) -> SourceLocation {
    SourceLocation::markdown(
        path.iter().map(|s| s.to_string()).collect(),
        Some((start, end)),
    )
    .unwrap()
}

#[test]
fn markdown_chunks_keep_the_heading_path_and_the_lines() {
    let guide = ["Guia"];
    let install = ["Guia", "Instalação"];
    let models = ["Guia", "Instalação", "Modelos Multilíngues"];
    let doc = document(
        DocumentType::Markdown,
        vec![
            section(
                &guide,
                vec![
                    block(ContentKind::Heading { level: 1 }, "Guia", md(&guide, 1, 1)),
                    block(ContentKind::Paragraph, prose("m1x", 2), md(&guide, 3, 3)),
                ],
            ),
            section(
                &install,
                vec![
                    block(
                        ContentKind::Heading { level: 2 },
                        "Instalação",
                        md(&install, 5, 5),
                    ),
                    block(ContentKind::Paragraph, prose("m2x", 12), md(&install, 7, 9)),
                    block(
                        ContentKind::Paragraph,
                        prose("m3x", 12),
                        md(&install, 11, 13),
                    ),
                ],
            ),
            section(
                &models,
                vec![
                    block(
                        ContentKind::Heading { level: 3 },
                        "Modelos Multilíngues",
                        md(&models, 15, 15),
                    ),
                    block(ContentKind::Paragraph, prose("m4x", 3), md(&models, 17, 17)),
                ],
            ),
        ],
    );
    let chunks = run(&doc);
    assert_provenance(&doc, &chunks);

    // The path is the chain of headings, and never mixes sections.
    let paths: Vec<_> = chunks.iter().map(|c| c.section_path.join(" > ")).collect();
    assert_eq!(paths.first().unwrap(), "Guia");
    assert_eq!(
        paths.last().unwrap(),
        "Guia > Instalação > Modelos Multilíngues"
    );
    assert!(chunks.len() >= 4, "{paths:?}");
    for chunk in &chunks {
        let SourceLocation::Markdown {
            heading_path,
            line_start,
            line_end,
        } = &chunk.location
        else {
            panic!("not a Markdown location");
        };
        assert_eq!(
            heading_path, &chunk.section_path,
            "the location's heading path"
        );
        let (start, end) = (line_start.unwrap(), line_end.unwrap());
        assert!(start >= 3 && end <= 17 && start <= end);
        // Headings are not repeated in the text, only in the path.
        assert!(!chunk.text.contains("Modelos Multilíngues"));
    }
    // The last section's chunk points at its own lines only.
    let last = chunks.last().unwrap();
    assert_eq!(last.location, md(&models, 17, 17));
}

#[test]
fn text_chunks_keep_their_offsets() {
    let mut blocks = Vec::new();
    let mut offset = 0u32;
    for n in 0..8 {
        let text = prose(&format!("m{n}x"), 4);
        let length = text.chars().count() as u32;
        blocks.push(block(
            ContentKind::Paragraph,
            text,
            SourceLocation::text(offset, offset + length).unwrap(),
        ));
        offset += length + 2;
    }
    let doc = document(
        DocumentType::Text,
        vec![DocumentSection::new(None, 0, vec![], blocks).unwrap()],
    );
    let chunks = run(&doc);
    assert_provenance(&doc, &chunks);
    assert!(chunks.len() > 2);
    let mut previous_start = 0;
    for chunk in &chunks {
        let SourceLocation::Text { start, end } = chunk.location else {
            panic!("not a text location");
        };
        assert!(start < end && end <= offset);
        assert!(start >= previous_start, "offsets move forward");
        previous_start = start;
        assert!(chunk.section_path.is_empty());
    }
    // Together the chunks cover the first and last paragraph of the source.
    let first = chunks.first().unwrap();
    let last = chunks.last().unwrap();
    assert!(matches!(
        first.location,
        SourceLocation::Text { start: 0, .. }
    ));
    let SourceLocation::Text { end, .. } = last.location else {
        unreachable!()
    };
    assert_eq!(end, offset - 2);
}

fn epub(chapter: u32, title: &str, section: Option<&str>) -> SourceLocation {
    SourceLocation::epub(chapter, Some(title.into()), section.map(String::from)).unwrap()
}

#[test]
fn epub_chunks_follow_chapters_and_never_cross_them() {
    // Both chapters share the same (empty) section path on purpose.
    let doc = document(
        DocumentType::Epub,
        vec![
            section(
                &[],
                vec![
                    block(
                        ContentKind::Paragraph,
                        prose("m1x", 3),
                        epub(1, "Capítulo 1", None),
                    ),
                    block(
                        ContentKind::Paragraph,
                        prose("m2x", 3),
                        epub(2, "Capítulo 2", None),
                    ),
                ],
            ),
            section(
                &["Capítulo 3", "Embeddings"],
                vec![
                    block(
                        ContentKind::Heading { level: 2 },
                        "Embeddings",
                        epub(3, "Capítulo 3", Some("Embeddings")),
                    ),
                    block(
                        ContentKind::Paragraph,
                        prose("m3x", 14),
                        epub(3, "Capítulo 3", Some("Embeddings")),
                    ),
                    block(
                        ContentKind::Paragraph,
                        prose("m4x", 14),
                        epub(3, "Capítulo 3", Some("Embeddings")),
                    ),
                ],
            ),
        ],
    );
    let chunks = run(&doc);
    assert_provenance(&doc, &chunks);

    let chapters: Vec<u32> = chunks
        .iter()
        .map(|c| match c.location {
            SourceLocation::Epub { chapter_index, .. } => chapter_index,
            ref other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        &chapters[..2],
        [1, 2],
        "chapters 1 and 2 are separate chunks"
    );
    assert!(chapters[2..].iter().all(|c| *c == 3));
    // The chapter and section survive in the path and in the location.
    let last = chunks.last().unwrap();
    assert_eq!(last.section_path, ["Capítulo 3", "Embeddings"]);
    assert_eq!(
        last.location,
        epub(3, "Capítulo 3", Some("Embeddings")),
        "same chapter and section merge to themselves"
    );
    // A chunk never holds text of two chapters.
    for chunk in &chunks {
        let holds = |m: &str| chunk.text.split_whitespace().any(|w| w == m);
        assert!(!(holds("m1x") && holds("m2x")));
    }
}

#[test]
fn lists_join_with_one_newline_and_pdf_keeps_blank_lines() {
    let items = |kind_doc: DocumentType| {
        let location = |n: u32| match kind_doc {
            DocumentType::Pdf => SourceLocation::pdf(1, 1, vec![]).unwrap(),
            _ => md(&["Lista"], n, n),
        };
        document(
            kind_doc,
            vec![section(
                &["Lista"],
                (0..3u32)
                    .map(|n| {
                        block(
                            ContentKind::ListItem {
                                ordered: false,
                                depth: 0,
                            },
                            format!("item{n} da lista"),
                            location(n + 1),
                        )
                    })
                    .collect(),
            )],
        )
    };
    let markdown = run(&items(DocumentType::Markdown));
    assert_eq!(markdown.len(), 1);
    assert_eq!(
        markdown[0].text,
        "item0 da lista\nitem1 da lista\nitem2 da lista"
    );
    let pdf = run(&items(DocumentType::Pdf));
    assert_eq!(
        pdf[0].text,
        "item0 da lista\n\nitem1 da lista\n\nitem2 da lista"
    );
}

#[test]
fn code_is_cut_by_lines_and_never_by_sentences() {
    // Each line holds a period followed by a space, where a sentence splitter would cut.
    let lines: Vec<String> = (0..40).map(|n| format!("let x{n} = a. b + {n};")).collect();
    let code = lines.join("\n");
    let doc = document(
        DocumentType::Markdown,
        vec![section(
            &["Código"],
            vec![block(
                ContentKind::CodeBlock {
                    language: Some("rust".into()),
                },
                code,
                md(&["Código"], 3, 45),
            )],
        )],
    );
    let chunks = run(&doc);
    assert!(chunks.len() > 1, "{} chunks", chunks.len());
    for chunk in &chunks {
        // Every line of a chunk is a whole original line, in order.
        for line in chunk.text.lines() {
            assert!(lines.iter().any(|l| l == line), "cut line: {line:?}");
        }
        assert!(chunk.token_count.unwrap() <= policy().max_tokens);
        assert_eq!(chunk.location, md(&["Código"], 3, 45));
    }
    // No line was lost or repeated (code is not given overlap).
    let joined: Vec<&str> = chunks.iter().flat_map(|c| c.text.lines()).collect();
    assert_eq!(joined, lines.iter().map(String::as_str).collect::<Vec<_>>());
}

#[test]
fn tables_are_cut_by_rows() {
    let rows: Vec<String> = (0..30)
        .map(|n| format!("| linha{n} | valor{n} |"))
        .collect();
    let doc = document(
        DocumentType::Markdown,
        vec![section(
            &["Tabela"],
            vec![block(
                ContentKind::Table {
                    header: vec!["a".into(), "b".into()],
                    rows: vec![],
                },
                rows.join("\n"),
                md(&["Tabela"], 2, 32),
            )],
        )],
    );
    let chunks = run(&doc);
    assert!(chunks.len() > 1);
    for chunk in &chunks {
        for line in chunk.text.lines() {
            assert!(rows.iter().any(|r| r == line), "cut row: {line:?}");
        }
    }
}

#[test]
fn pdf_documents_keep_pages_and_boxes() {
    let boxed = |page: u32| PageBox {
        page,
        bbox: BoundingBox {
            left: 1.0,
            top: 2.0,
            right: 3.0,
            bottom: 4.0,
        },
    };
    let pdf = |page: u32| SourceLocation::pdf(page, page, vec![boxed(page)]).unwrap();
    let doc = document(
        DocumentType::Pdf,
        vec![section(
            &["1 Introdução"],
            (1..=6u32)
                .map(|page| {
                    block(
                        ContentKind::Paragraph,
                        prose(&format!("m{page}x"), 4),
                        pdf(page),
                    )
                })
                .collect(),
        )],
    );
    let chunks = run(&doc);
    assert_provenance(&doc, &chunks);
    for chunk in &chunks {
        let (start, end) = chunk.location.page_range().unwrap();
        assert!(1 <= start && start <= end && end <= 6);
        assert!(
            chunk
                .location
                .boxes()
                .iter()
                .all(|b| (start..=end).contains(&b.page))
        );
        assert!(!chunk.location.boxes().is_empty());
    }
}

fn record(row: u32, name: &str) -> ContentBlock {
    block(
        ContentKind::Record {
            fields: vec![
                RecordField {
                    name: "Nome".into(),
                    value: name.into(),
                },
                RecordField {
                    name: "Cidade".into(),
                    value: "Recife".into(),
                },
            ],
        },
        format!("Registro {row}:\nNome: {name}\nCidade: Recife"),
        SourceLocation::csv(row, row).unwrap(),
    )
}

#[test]
fn csv_documents_go_through_the_record_chunker() {
    let blocks: Vec<_> = (1..=3).map(|n| record(n, &format!("Pessoa{n}"))).collect();
    let dataset = DatasetMetadata {
        columns: ["Nome", "Cidade"]
            .iter()
            .map(|name| DatasetColumn {
                name: name.to_string(),
                kind: ColumnKind::Text,
            })
            .collect(),
        delimiter: ';',
        has_header: true,
        row_count: Some(3),
    };
    let metadata = DocumentMetadata {
        dataset: Some(dataset),
        ..Default::default()
    };
    let with_dataset = ParsedDocument::new(
        DocumentType::Csv,
        metadata,
        vec![],
        vec![DocumentSection::new(None, 0, vec![], blocks.clone()).unwrap()],
        vec![],
    )
    .unwrap();
    let chunks = MultiFormatChunker.chunk(
        &with_dataset,
        &context(),
        &ChunkPolicy::default(),
        &WordCounter,
    );
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].location, SourceLocation::csv(1, 3).unwrap());
    assert_eq!(chunks[0].metadata.columns, ["Nome", "Cidade"]);
    assert_eq!(chunks[0].metadata.document_type, DocumentType::Csv);
    assert_eq!(chunks[0].document_id, 42);
    assert!(chunks[0].text.contains("Arquivo: guia.md"));

    // Without dataset metadata the columns come from the first record.
    let without = document(
        DocumentType::Csv,
        vec![DocumentSection::new(None, 0, vec![], blocks).unwrap()],
    );
    let chunks =
        MultiFormatChunker.chunk(&without, &context(), &ChunkPolicy::default(), &WordCounter);
    assert_eq!(chunks[0].metadata.columns, ["Nome", "Cidade"]);
}

#[test]
fn a_document_without_text_has_no_chunks() {
    let doc = document(
        DocumentType::Text,
        vec![section(
            &[],
            vec![block(
                ContentKind::Paragraph,
                "   ",
                SourceLocation::text(0, 3).unwrap(),
            )],
        )],
    );
    assert!(run(&doc).is_empty());
    assert!(
        run(&document(DocumentType::Markdown, vec![])).is_empty(),
        "nothing in, nothing out"
    );
}

#[test]
fn the_output_is_deterministic() {
    let doc = document(
        DocumentType::Markdown,
        vec![section(
            &["Guia"],
            (0..6u32)
                .map(|n| {
                    block(
                        ContentKind::Paragraph,
                        prose(&format!("m{n}x"), 6),
                        md(&["Guia"], n * 3 + 1, n * 3 + 2),
                    )
                })
                .collect(),
        )],
    );
    assert_eq!(run(&doc), run(&doc));
    assert_eq!(MultiFormatChunker.version(), MultiFormatChunker::VERSION);
}
