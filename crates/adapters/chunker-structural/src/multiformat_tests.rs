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

/// A value with no spaces (a base64 blob, minified JSON, a long URL) in a paragraph of a text
/// document: it is cut by characters, every chunk stays within the model's limit, nothing is lost,
/// and it takes linear time (a 2 MB value took 45 s when each piece copied the rest).
#[test]
fn an_unbroken_value_in_prose_is_cut_into_chunks_that_fit() {
    use crate::HeuristicTokenCounter;
    let blob = "QUJD".repeat(512 * 1024); // 2 MB of base64, no whitespace
    let text = format!("Antes do bloco. {blob} Depois do bloco.");
    let document = document(
        DocumentType::Markdown,
        vec![section(
            &["Dados"],
            vec![block(
                ContentKind::Paragraph,
                text,
                SourceLocation::markdown(vec!["Dados".into()], Some((3, 3))).unwrap(),
            )],
        )],
    );
    let policy = ChunkPolicy::default();
    let started = std::time::Instant::now();
    let chunks = MultiFormatChunker.chunk(&document, &context(), &policy, &HeuristicTokenCounter);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    assert!(chunks.len() > 100);
    for chunk in &chunks {
        let tokens = HeuristicTokenCounter.count(&chunk.text);
        assert!(
            tokens <= policy.max_tokens + policy.overlap_tokens,
            "{tokens} tokens"
        );
        assert_eq!(
            chunk.location, chunks[0].location,
            "the location is the block's"
        );
    }
    let all: String = chunks.iter().map(|c| c.text.as_str()).collect();
    assert!(all.starts_with("Antes do bloco."));
    assert!(all.contains("Depois do bloco."));
    assert!(
        all.matches("QUJD").count() >= 512 * 1024,
        "the blob is all there"
    );
}

fn xlsx_record(sheet: u32, name: &str, row: u32, who: &str) -> ContentBlock {
    block(
        ContentKind::Record {
            fields: vec![RecordField {
                name: "Nome".into(),
                value: who.into(),
            }],
        },
        format!("Registro {row}:\nNome: {who}"),
        SourceLocation::xlsx(sheet, name.into(), row, row).unwrap(),
    )
}

#[test]
fn xlsx_documents_are_chunked_sheet_by_sheet_with_their_own_columns() {
    let first: Vec<_> = (2..=4)
        .map(|r| xlsx_record(1, "Clientes", r, &format!("Pessoa{r}")))
        .collect();
    let mut second = vec![block(
        ContentKind::Record {
            fields: vec![RecordField {
                name: "Produto".into(),
                value: "Cadeira".into(),
            }],
        },
        "Registro 7:\nProduto: Cadeira",
        SourceLocation::xlsx(2, "Estoque".into(), 7, 7).unwrap(),
    )];
    second.push(block(
        ContentKind::Record {
            fields: vec![RecordField {
                name: "Produto".into(),
                value: "Mesa".into(),
            }],
        },
        "Registro 9:\nProduto: Mesa",
        SourceLocation::xlsx(2, "Estoque".into(), 9, 9).unwrap(),
    ));
    let workbook = document(
        DocumentType::Xlsx,
        vec![section(&["Clientes"], first), section(&["Estoque"], second)],
    );
    let chunks =
        MultiFormatChunker.chunk(&workbook, &context(), &ChunkPolicy::default(), &WordCounter);
    assert_eq!(chunks.len(), 2, "one chunk per sheet here");
    assert_eq!(
        chunks[0].location,
        SourceLocation::xlsx(1, "Clientes".into(), 2, 4).unwrap()
    );
    assert_eq!(chunks[0].section_path, ["Clientes"]);
    assert_eq!(chunks[0].metadata.columns, ["Nome"]);
    assert!(
        chunks[0]
            .text
            .starts_with("Arquivo: guia.md\nPlanilha: Clientes\nColunas: Nome\nLinhas 2–4\n\n")
    );
    assert_eq!(
        chunks[1].location,
        SourceLocation::xlsx(2, "Estoque".into(), 7, 9).unwrap()
    );
    assert_eq!(chunks[1].metadata.columns, ["Produto"]);
    assert_eq!(chunks[1].metadata.document_type, DocumentType::Xlsx);
    assert_eq!(
        chunks.iter().map(|c| c.index).collect::<Vec<_>>(),
        [0, 1],
        "indexes run across sheets"
    );
    assert!(chunks.iter().all(|c| c.location.validate().is_ok()));
}

#[test]
fn a_large_sheet_splits_into_chunks_that_keep_the_sheet_and_number_on() {
    let rows: Vec<_> = (2..=40)
        .map(|r| xlsx_record(1, "Dados", r, &format!("Pessoa{r}")))
        .collect();
    let one = document(DocumentType::Xlsx, vec![section(&["Dados"], rows)]);
    let chunks = run(&one);
    assert!(chunks.len() > 1);
    for (i, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.index, i as u32);
        assert!(chunk.text.contains("Planilha: Dados"));
        assert!(matches!(
            chunk.location,
            SourceLocation::Xlsx { sheet_index: 1, .. }
        ));
    }
}

#[test]
fn docx_chunks_keep_the_heading_path_and_the_paragraphs() {
    let docx = |path: &[&str], n: u32| {
        SourceLocation::docx(path.iter().map(|s| s.to_string()).collect(), Some((n, n))).unwrap()
    };
    let doc = document(
        DocumentType::Docx,
        vec![
            section(
                &["Objeto"],
                vec![
                    block(
                        ContentKind::Heading { level: 1 },
                        "Objeto",
                        docx(&["Objeto"], 1),
                    ),
                    block(
                        ContentKind::Paragraph,
                        format!("{} fim.", prose("alfa", 2)),
                        docx(&["Objeto"], 2),
                    ),
                ],
            ),
            section(
                &["Objeto", "Prazos"],
                vec![
                    block(
                        ContentKind::Heading { level: 2 },
                        "Prazos",
                        docx(&["Objeto", "Prazos"], 3),
                    ),
                    block(
                        ContentKind::Paragraph,
                        prose("beta", 2),
                        docx(&["Objeto", "Prazos"], 4),
                    ),
                ],
            ),
        ],
    );
    let chunks = run(&doc);
    assert_provenance(&doc, &chunks);
    assert!(
        chunks
            .iter()
            .all(|c| matches!(c.location, SourceLocation::Docx { .. }))
    );
    let beta = chunks
        .iter()
        .find(|c| c.text.contains("beta"))
        .expect("the second section is chunked");
    assert_eq!(beta.section_path, ["Objeto", "Prazos"]);
}

#[test]
fn docx_table_rows_are_never_cut_and_keep_the_heading_path() {
    let docx = |n: u32| SourceLocation::docx(vec!["Equipe".into()], Some((n, n))).unwrap();
    let mut blocks = vec![block(ContentKind::Heading { level: 1 }, "Equipe", docx(1))];
    let mut rows = Vec::new();
    for n in 0..12u32 {
        let text = format!(
            "Tabela: Nome | Cidade | Cargo\nNome: Pessoa{n}\nCidade: Fortaleza\nCargo: Engenheiro"
        );
        rows.push(text.clone());
        blocks.push(block(
            ContentKind::Record {
                fields: vec![
                    RecordField {
                        name: "Nome".into(),
                        value: format!("Pessoa{n}"),
                    },
                    RecordField {
                        name: "Cidade".into(),
                        value: "Fortaleza".into(),
                    },
                    RecordField {
                        name: "Cargo".into(),
                        value: "Engenheiro".into(),
                    },
                ],
            },
            text,
            docx(n + 2),
        ));
    }
    let doc = document(DocumentType::Docx, vec![section(&["Equipe"], blocks)]);
    let chunks = run(&doc);
    assert!(chunks.len() > 1, "the rows do not fit one chunk");
    for row in &rows {
        assert!(
            chunks.iter().any(|c| c.text.contains(row.as_str())),
            "a row was cut or lost: {row}"
        );
    }
    for chunk in &chunks {
        assert_eq!(chunk.section_path, ["Equipe"]);
        assert!(matches!(chunk.location, SourceLocation::Docx { .. }));
        assert!(chunk.text.contains("Tabela: Nome | Cidade | Cargo"));
    }
}

fn docx_at(path: &[&str], n: u32, table: Option<u32>) -> SourceLocation {
    SourceLocation::docx_table(
        path.iter().map(|s| s.to_string()).collect(),
        Some((n, n)),
        table,
    )
    .unwrap()
}

fn docx_row(path: &[&str], n: u32, table: u32, who: &str) -> ContentBlock {
    block(
        ContentKind::Record {
            fields: vec![RecordField {
                name: "Nome".into(),
                value: who.into(),
            }],
        },
        format!("Tabela {table}: Nome\nNome: {who}"),
        docx_at(path, n, Some(table)),
    )
}

#[test]
fn a_docx_chunk_names_the_table_it_includes_unless_there_are_two() {
    let path = ["Equipe"];
    let only_rows = document(
        DocumentType::Docx,
        vec![section(
            &path,
            vec![
                block(
                    ContentKind::Heading { level: 1 },
                    "Equipe",
                    docx_at(&path, 1, None),
                ),
                docx_row(&path, 2, 1, "Ana"),
                docx_row(&path, 3, 1, "Bia"),
            ],
        )],
    );
    let chunks = run(&only_rows);
    assert_eq!(chunks.len(), 1);
    let SourceLocation::Docx {
        table,
        heading_path,
        ..
    } = &chunks[0].location
    else {
        panic!("a DOCX location");
    };
    assert_eq!(heading_path, &["Equipe"]);
    // A heading only names the section; the chunk's location is that of its content.
    assert_eq!(*table, Some(1));

    // Rows only (no prose) in one chunk: the chunk is that table.
    let rows_only = document(
        DocumentType::Docx,
        vec![section(
            &path,
            vec![docx_row(&path, 2, 1, "Ana"), docx_row(&path, 3, 1, "Bia")],
        )],
    );
    let chunks = run(&rows_only);
    assert_eq!(chunks.len(), 1);
    assert_eq!(
        chunks[0].location,
        docx_at(&path, 2, Some(1))
            .merge(&docx_at(&path, 3, Some(1)))
            .unwrap()
    );
    assert_eq!(chunks[0].location.label(), "Equipe, tabela 1");

    // Two tables in one chunk: neither.
    let two = document(
        DocumentType::Docx,
        vec![section(
            &path,
            vec![docx_row(&path, 2, 1, "Ana"), docx_row(&path, 3, 2, "Bia")],
        )],
    );
    let chunks = run(&two);
    assert_eq!(chunks.len(), 1);
    assert!(matches!(
        chunks[0].location,
        SourceLocation::Docx { table: None, .. }
    ));
}

#[test]
fn docx_lists_and_sections_never_mix_in_a_chunk() {
    let backend = ["Arquitetura", "Backend"];
    let frontend = ["Arquitetura", "Frontend"];
    let item = |path: &[&str], n: u32, text: &str| {
        block(
            ContentKind::ListItem {
                ordered: false,
                depth: 0,
            },
            text,
            docx_at(path, n, None),
        )
    };
    let doc = document(
        DocumentType::Docx,
        vec![
            section(
                &backend,
                vec![item(&backend, 1, "alfa um"), item(&backend, 2, "alfa dois")],
            ),
            section(
                &frontend,
                vec![
                    item(&frontend, 3, "beta um"),
                    item(&frontend, 4, "beta dois"),
                ],
            ),
        ],
    );
    let chunks = run(&doc);
    assert_provenance(&doc, &chunks);
    for chunk in &chunks {
        let alfa = chunk.text.contains("alfa");
        let beta = chunk.text.contains("beta");
        assert!(alfa != beta, "one section per chunk: {:?}", chunk.text);
        let expected: &[&str] = if alfa { &backend } else { &frontend };
        assert_eq!(chunk.section_path, expected);
        assert_eq!(chunk.location.label(), expected.join(" › "));
    }
    assert!(
        chunks[0].text.contains("alfa um\nalfa dois"),
        "items of one list stay together"
    );
}

#[test]
fn an_xlsx_sheet_with_a_title_row_takes_its_columns_from_the_records() {
    let loc = |row: u32| SourceLocation::xlsx(1, "Relatório".into(), row, row).unwrap();
    let record = |row: u32, who: &str| {
        block(
            ContentKind::Record {
                fields: vec![
                    RecordField {
                        name: "Produto".into(),
                        value: who.into(),
                    },
                    RecordField {
                        name: "Produto (2)".into(),
                        value: "x".into(),
                    },
                ],
            },
            format!("Registro {row}:\nProduto: {who}\nProduto (2): x"),
            loc(row),
        )
    };
    let doc = document(
        DocumentType::Xlsx,
        vec![section(
            &["Relatório"],
            vec![
                block(
                    ContentKind::Paragraph,
                    "Linha 1: Relatório de vendas",
                    loc(1),
                ),
                record(4, "Notebook"),
                record(5, "Monitor"),
                block(ContentKind::Paragraph, "Linha 8: Valores em reais", loc(8)),
            ],
        )],
    );
    let chunks = MultiFormatChunker.chunk(&doc, &context(), &ChunkPolicy::default(), &WordCounter);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].metadata.columns, ["Produto", "Produto (2)"]);
    assert_eq!(
        chunks[0].location,
        SourceLocation::xlsx(1, "Relatório".into(), 1, 8).unwrap()
    );
    assert_eq!(chunks[0].location.label(), "Relatório, linhas 1–8");
    for part in [
        "Planilha: Relatório",
        "Linha 1: Relatório de vendas",
        "Registro 4:",
        "Linha 8: Valores em reais",
    ] {
        assert!(chunks[0].text.contains(part), "{part}");
    }
}

fn sheet_record(sheet: u32, name: &str, row: u32, fields: &[(&str, &str)]) -> ContentBlock {
    let mut text = format!("Registro {row}:");
    for (column, value) in fields.iter().filter(|(_, v)| !v.is_empty()) {
        text.push_str(&format!("\n{column}: {value}"));
    }
    block(
        ContentKind::Record {
            fields: fields
                .iter()
                .map(|(name, value)| RecordField {
                    name: name.to_string(),
                    value: value.to_string(),
                })
                .collect(),
        },
        text,
        SourceLocation::xlsx(sheet, name.into(), row, row).unwrap(),
    )
}

fn rows_of(chunk: &DocumentChunk) -> (u32, u32) {
    match &chunk.location {
        SourceLocation::Xlsx {
            row_start, row_end, ..
        } => (*row_start, *row_end),
        other => panic!("not an XLSX location: {other:?}"),
    }
}

#[test]
fn a_large_sheet_is_split_into_contiguous_chunks_that_each_carry_the_columns() {
    let records: Vec<_> = (2..=121u32)
        .map(|r| {
            sheet_record(
                1,
                "Clientes",
                r,
                &[
                    ("Cliente", &format!("Cliente{r}")),
                    ("Cidade", "Fortaleza"),
                    ("Status", if r % 2 == 0 { "Ativo" } else { "Inativo" }),
                ],
            )
        })
        .collect();
    let doc = document(DocumentType::Xlsx, vec![section(&["Clientes"], records)]);
    let chunks = run(&doc);
    assert!(chunks.len() > 3, "{} chunks", chunks.len());
    let mut next_row = 2;
    for (i, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.index, i as u32);
        // Header context on every chunk, never bare values.
        assert!(
            chunk
                .text
                .contains("Planilha: Clientes\nColunas: Cliente, Cidade, Status\n"),
            "{}",
            chunk.text
        );
        assert_eq!(chunk.metadata.columns, ["Cliente", "Cidade", "Status"]);
        assert_eq!(chunk.section_path, ["Clientes"]);
        assert!(
            chunk.token_count.unwrap() <= policy().max_tokens,
            "{}",
            chunk.text
        );
        // Rows are contiguous, never repeated and never skipped.
        let (start, end) = rows_of(chunk);
        assert_eq!(start, next_row, "chunk {i} starts where the last ended");
        assert!(end >= start);
        next_row = end + 1;
        // The preamble's range is the location's range.
        let range = if start == end {
            format!("Linha {start}\n")
        } else {
            format!("Linhas {start}–{end}\n")
        };
        assert!(chunk.text.contains(&range), "{range} in {}", chunk.text);
        // Every record in the chunk reads "Coluna: valor".
        assert!(chunk.text.contains("Cliente: Cliente"));
        assert!(!chunk.text.lines().any(|l| l.trim() == "Fortaleza"));
    }
    assert_eq!(next_row, 122, "all 120 rows are covered");
}

#[test]
fn worksheets_never_share_a_chunk_and_number_their_chunks_in_one_sequence() {
    let one: Vec<_> = (2..=3)
        .map(|r| {
            sheet_record(
                1,
                "Clientes",
                r,
                &[("Cliente", "Ana"), ("Cidade", "Recife")],
            )
        })
        .collect();
    let two: Vec<_> = (2..=3)
        .map(|r| sheet_record(2, "Pedidos", r, &[("Pedido", "P-1"), ("Valor", "10")]))
        .collect();
    let doc = document(
        DocumentType::Xlsx,
        vec![section(&["Clientes"], one), section(&["Pedidos"], two)],
    );
    let chunks = MultiFormatChunker.chunk(&doc, &context(), &ChunkPolicy::default(), &WordCounter);
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].location.label(), "Clientes, linhas 2–3");
    assert_eq!(chunks[1].location.label(), "Pedidos, linhas 2–3");
    assert_eq!(chunks[0].metadata.columns, ["Cliente", "Cidade"]);
    assert_eq!(chunks[1].metadata.columns, ["Pedido", "Valor"]);
    assert!(!chunks[0].text.contains("Pedido"));
    assert!(!chunks[1].text.contains("Cliente"));
    assert_eq!(chunks.iter().map(|c| c.index).collect::<Vec<_>>(), [0, 1]);
}

#[test]
fn mixed_data_keeps_every_value_with_its_column_and_leaves_empty_cells_out() {
    let doc = document(
        DocumentType::Xlsx,
        vec![section(
            &["Mix"],
            vec![
                sheet_record(
                    1,
                    "Mix",
                    2,
                    &[
                        ("Nome", "João"),
                        ("Idade", "30"),
                        ("Início", "2024-03-01"),
                        ("Nota", ""),
                    ],
                ),
                sheet_record(
                    1,
                    "Mix",
                    3,
                    &[
                        ("Nome", "Maria"),
                        ("Idade", ""),
                        ("Início", ""),
                        ("Nota", "Ativa em 2024-05-02, 1250.5 pontos"),
                    ],
                ),
            ],
        )],
    );
    let chunks = MultiFormatChunker.chunk(&doc, &context(), &ChunkPolicy::default(), &WordCounter);
    assert_eq!(chunks.len(), 1);
    let text = &chunks[0].text;
    assert!(text.contains("Nome: João\nIdade: 30\nInício: 2024-03-01"));
    assert!(text.contains("Nome: Maria\nNota: Ativa em 2024-05-02, 1250.5 pontos"));
    assert!(
        !text.contains("Nota: \n") && !text.contains("Idade: \n"),
        "no empty cells: {text}"
    );
    assert_eq!(rows_of(&chunks[0]), (2, 3));
}

#[test]
fn a_record_larger_than_a_chunk_is_split_by_fields_and_every_part_keeps_the_context() {
    let long = "palavra ".repeat(60);
    let doc = document(
        DocumentType::Xlsx,
        vec![section(
            &["Notas"],
            vec![sheet_record(
                1,
                "Notas",
                7,
                &[("Título", "Reunião"), ("Texto", &long), ("Autor", "Ana")],
            )],
        )],
    );
    let chunks = run(&doc);
    assert!(chunks.len() > 1);
    for chunk in &chunks {
        assert!(chunk.text.contains("Planilha: Notas\n"));
        assert!(chunk.text.contains("Linha 7\n"));
        assert_eq!(rows_of(chunk), (7, 7));
        assert!(chunk.text.contains("(parte "), "{}", chunk.text);
        assert!(chunk.token_count.unwrap() <= policy().max_tokens);
    }
}

#[test]
fn a_sheet_without_records_invents_no_columns() {
    let loc = |row: u32| SourceLocation::xlsx(1, "Notas".into(), row, row).unwrap();
    let doc = document(
        DocumentType::Xlsx,
        vec![section(
            &["Notas"],
            vec![
                block(
                    ContentKind::Paragraph,
                    "Linha 1: Relatório de vendas",
                    loc(1),
                ),
                block(ContentKind::Paragraph, "Linha 2: Valores em reais", loc(2)),
            ],
        )],
    );
    let chunks = MultiFormatChunker.chunk(&doc, &context(), &ChunkPolicy::default(), &WordCounter);
    assert_eq!(chunks.len(), 1);
    assert!(!chunks[0].text.contains("Colunas:"));
    assert!(chunks[0].metadata.columns.is_empty());
    assert!(chunks[0].text.contains("Planilha: Notas"));
    assert_eq!(chunks[0].location.label(), "Notas, linhas 1–2");
}
