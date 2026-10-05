//! The content pipeline (parse → normalize → chunk) with every real adapter: the chunks of each
//! format keep their provenance, and the PDF ones are what the legacy chunker made.

mod support;

use std::{collections::HashSet, path::PathBuf, sync::Arc};

use nlmx_application::{
    ports::{Chunker, DocumentEngine, DocumentNormalizer, DocumentSource, StructureAnalyzer},
    services::{
        parsing::{ParserRegistry, PdfDocumentParser},
        pipeline::{ContentPipeline, ProcessedDocument},
    },
};
use nlmx_chunker_structural::{HeuristicTokenCounter, MultiFormatChunker, StructuralChunker};
use nlmx_domain::{
    document_type::DocumentType,
    ingestion::{ChunkPolicy, PageLayout},
    source::SourceLocation,
};
use nlmx_normalizer_text::TextNormalizer;
use nlmx_parser_epub::EpubDocumentParser;
use nlmx_parser_office::{DocxDocumentParser, XlsxDocumentParser};
use nlmx_parser_text::{CsvDocumentParser, MarkdownDocumentParser, TextDocumentParser};
use nlmx_pdf_pdfium::PdfiumDocumentEngine;
use nlmx_structure_heuristic::HeuristicStructureAnalyzer;
use nlmx_testing::FakeDocumentNormalizer;
use support::{fixture, root, temp_dir};

/// PDFium can be initialized once per process, so every test shares one engine.
fn engine() -> Arc<PdfiumDocumentEngine> {
    static ENGINE: std::sync::OnceLock<Arc<PdfiumDocumentEngine>> = std::sync::OnceLock::new();
    ENGINE
        .get_or_init(|| {
            Arc::new(PdfiumDocumentEngine::from_default_location().expect("make bootstrap"))
        })
        .clone()
}

fn pipeline_with(normalizer: Arc<dyn DocumentNormalizer>, policy: ChunkPolicy) -> ContentPipeline {
    let parsers = ParserRegistry::new()
        .with(Arc::new(PdfDocumentParser::new(
            engine(),
            Arc::new(HeuristicStructureAnalyzer),
        )))
        .with(Arc::new(MarkdownDocumentParser))
        .with(Arc::new(TextDocumentParser))
        .with(Arc::new(CsvDocumentParser))
        .with(Arc::new(EpubDocumentParser::default()))
        .with(Arc::new(DocxDocumentParser::default()))
        .with(Arc::new(XlsxDocumentParser::default()));
    ContentPipeline {
        parsers,
        normalizer,
        chunker: Arc::new(MultiFormatChunker),
        tokens: Arc::new(HeuristicTokenCounter),
        policy,
    }
}

fn pipeline() -> ContentPipeline {
    pipeline_with(Arc::new(TextNormalizer), ChunkPolicy::default())
}

fn text_fixture(name: &str) -> PathBuf {
    root()
        .join("crates/adapters/parser-text/tests/fixtures")
        .join(name)
}

fn epub_fixture(name: &str) -> PathBuf {
    root()
        .join("crates/adapters/parser-epub/tests/fixtures")
        .join(name)
}

async fn run(pipeline: &ContentPipeline, path: &PathBuf) -> ProcessedDocument {
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    pipeline
        .run(&DocumentSource::from_path(path), 11, Some(&name))
        .await
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Lowercase alphanumeric words, so markup (`**`, `|`, `1.`) does not matter.
fn words(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// What every chunk of every format must satisfy.
fn check_chunks(processed: &ProcessedDocument, name: &str) {
    assert!(!processed.chunks.is_empty(), "{name}: no chunks");
    for (position, chunk) in processed.chunks.iter().enumerate() {
        assert_eq!(chunk.index as usize, position, "{name}");
        assert_eq!(chunk.document_id, 11, "{name}");
        assert_eq!(chunk.chunk_id, None, "{name}");
        assert!(chunk.token_count.is_some_and(|n| n > 0), "{name}");
        assert_eq!(
            chunk.location.validate(),
            Ok(()),
            "{name}: {:?}",
            chunk.location
        );
        assert_eq!(
            chunk.location.document_type(),
            processed.document_type,
            "{name}"
        );
        assert_eq!(
            chunk.metadata.document_type, processed.document_type,
            "{name}"
        );
        assert_eq!(chunk.metadata.file_name.as_deref(), Some(name), "{name}");
        assert_eq!(chunk.content_hash.len(), 64, "{name}");
    }
}

/// A chunk's page range and section path.
type PageAndPath = (Option<(u32, u32)>, Vec<String>);

// ── PDF ──────────────────────────────────────────────────────────────────────

async fn legacy_drafts(
    engine: &PdfiumDocumentEngine,
    name: &str,
) -> (Vec<nlmx_domain::ingestion::ChunkDraft>, u32) {
    let handle = engine.open(&fixture(name)).await.unwrap();
    let metadata = engine.metadata(handle).await.unwrap();
    let mut pages = Vec::new();
    for number in 1..=metadata.page_count {
        let info = engine.page_info(handle, number).await.unwrap();
        let spans = if info.has_text {
            engine.text_spans(handle, number).await.unwrap()
        } else {
            Vec::new()
        };
        pages.push(PageLayout {
            number,
            width: info.width,
            height: info.height,
            has_text: info.has_text,
            char_count: info.char_count,
            spans,
        });
    }
    engine.close(handle).await.unwrap();
    let structured = HeuristicStructureAnalyzer.analyze(&pages);
    let drafts =
        StructuralChunker.chunk(&structured, &ChunkPolicy::default(), &HeuristicTokenCounter);
    (drafts, metadata.page_count)
}

#[tokio::test]
async fn pdf_chunks_are_what_the_legacy_chunker_made() {
    // Without the normalizer, so only the chunking is compared.
    let pipeline = pipeline_with(
        Arc::new(FakeDocumentNormalizer::default()),
        ChunkPolicy::default(),
    );
    let engine = engine();
    for name in [
        "report.pdf",
        "text.pdf",
        "unicode.pdf",
        "rotated.pdf",
        "mixed.pdf",
        "large.pdf",
    ] {
        let (drafts, page_count) = legacy_drafts(&engine, name).await;
        let processed = run(&pipeline, &fixture(name)).await;
        check_chunks(&processed, name);

        assert_eq!(processed.chunks.len(), drafts.len(), "{name}");
        for (chunk, draft) in processed.chunks.iter().zip(&drafts) {
            assert_eq!(chunk.text, draft.text, "{name} #{}", draft.index);
            assert_eq!(chunk.section_path, draft.section_path, "{name}");
            assert_eq!(chunk.content_hash, draft.content_hash, "{name}");
            assert_eq!(chunk.token_count, Some(draft.token_count), "{name}");
            assert_eq!(
                chunk.location,
                SourceLocation::Pdf {
                    page_start: draft.page_start,
                    page_end: draft.page_end,
                    boxes: draft.boxes.clone(),
                },
                "{name} #{}",
                draft.index
            );
            // The provenance the viewer needs: valid pages, boxes inside the chunk's pages.
            let (first, last) = chunk.location.page_range().unwrap();
            assert!(1 <= first && first <= last && last <= page_count, "{name}");
            assert!(
                chunk
                    .location
                    .boxes()
                    .iter()
                    .all(|b| (first..=last).contains(&b.page)),
                "{name}"
            );
        }
        assert_eq!(processed.pages.len() as u32, page_count, "{name}");
    }
}

#[tokio::test]
async fn the_normalizer_does_not_move_pdf_provenance() {
    let raw = pipeline_with(
        Arc::new(FakeDocumentNormalizer::default()),
        ChunkPolicy::default(),
    );
    let normalized = pipeline();
    for name in ["report.pdf", "unicode.pdf", "large.pdf"] {
        let a = run(&raw, &fixture(name)).await;
        let b = run(&normalized, &fixture(name)).await;
        check_chunks(&b, name);
        let locations = |p: &ProcessedDocument| -> Vec<PageAndPath> {
            p.chunks
                .iter()
                .map(|c| (c.location.page_range(), c.section_path.clone()))
                .collect()
        };
        assert_eq!(locations(&a).len(), locations(&b).len(), "{name}");
        assert_eq!(locations(&a), locations(&b), "{name}");
    }
}

#[tokio::test]
async fn a_scanned_pdf_has_pages_and_no_chunks() {
    let processed = run(&pipeline(), &fixture("scanned.pdf")).await;
    assert!(processed.needs_ocr);
    assert!(processed.chunks.is_empty());
    assert!(!processed.pages.is_empty());
}

// ── Markdown ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn markdown_chunks_keep_the_heading_path_and_the_lines() {
    let path = text_fixture("guia.md");
    let source = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = source.lines().collect();
    let processed = run(&pipeline(), &path).await;
    check_chunks(&processed, "guia.md");

    let paths: Vec<_> = processed
        .chunks
        .iter()
        .map(|c| c.section_path.clone())
        .collect();
    let guia = "Guia de Instalação".to_string();
    assert!(
        paths.contains(&vec![guia.clone(), "Requisitos".into()]),
        "{paths:?}"
    );
    assert!(
        paths.contains(&vec![guia.clone(), "Requisitos".into(), "Detalhes".into()]),
        "{paths:?}"
    );
    assert!(
        paths.contains(&vec![guia.clone(), "Uso".into()]),
        "{paths:?}"
    );

    for chunk in &processed.chunks {
        let SourceLocation::Markdown {
            heading_path,
            line_start: Some(start),
            line_end: Some(end),
        } = &chunk.location
        else {
            panic!("a Markdown chunk with lines: {:?}", chunk.location);
        };
        // The path is the section's, the lines are inside the file...
        assert!(chunk.section_path.starts_with(heading_path), "{chunk:?}");
        assert!(*start >= 1 && start <= end && *end as usize <= lines.len());
        // ... and the text comes from them.
        let region = lines[*start as usize - 1..*end as usize].join("\n");
        let region_words = words(&region);
        for word in words(&chunk.text) {
            assert!(
                region_words.contains(&word),
                "{word:?} is not in lines {start}-{end}"
            );
        }
    }

    // Code and table survive as blocks (not cut up by sentences).
    let all = processed
        .chunks
        .iter()
        .map(|c| c.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains("make bootstrap\nmake dev"), "{all}");
    assert!(all.contains("PDFium"), "{all}");
    assert_eq!(
        processed.metadata.title.as_deref(),
        Some("Guia de Instalação")
    );
}

// ── TXT ──────────────────────────────────────────────────────────────────────

fn slice_chars(text: &str, start: u32, end: u32) -> String {
    text.chars()
        .skip(start as usize)
        .take((end - start) as usize)
        .collect()
}

async fn check_text_offsets(path: &PathBuf) -> ProcessedDocument {
    let source = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
    let processed = run(&pipeline(), path).await;
    check_chunks(&processed, &path.file_name().unwrap().to_string_lossy());
    for chunk in &processed.chunks {
        let SourceLocation::Text { start, end } = chunk.location else {
            panic!("a TXT chunk has offsets: {:?}", chunk.location);
        };
        assert!(end as usize <= source.chars().count());
        let region = words(&slice_chars(&source, start, end));
        for word in words(&chunk.text) {
            assert!(
                region.contains(&word),
                "{word:?} is not in [{start}, {end})"
            );
        }
    }
    processed
}

#[tokio::test]
async fn txt_chunks_keep_their_offsets() {
    let processed = check_text_offsets(&text_fixture("notas.txt")).await;
    assert!(processed.chunks.iter().all(|c| c.section_path.is_empty()));
}

#[tokio::test]
async fn a_long_txt_paragraph_does_not_inherit_the_whole_file() {
    let dir = temp_dir("pipeline-txt");
    let path = dir.join("longo.txt");
    // One paragraph of ~12 000 characters with no blank line.
    let lines: Vec<String> = (0..150)
        .map(|n| {
            format!("Linha {n} do texto contínuo que descreve o assunto número {n} com detalhes.")
        })
        .collect();
    std::fs::write(&path, lines.join("\n")).unwrap();

    let processed = check_text_offsets(&path).await;
    let total = std::fs::read_to_string(&path).unwrap().chars().count() as u32;
    let ranges: HashSet<(u32, u32)> = processed
        .chunks
        .iter()
        .map(|c| match c.location {
            SourceLocation::Text { start, end } => (start, end),
            _ => unreachable!(),
        })
        .collect();
    assert!(processed.chunks.len() > 5, "{}", processed.chunks.len());
    assert!(ranges.len() > 5, "chunks have their own offsets");
    assert!(
        ranges.iter().all(|(s, e)| e - s < total / 2),
        "no chunk claims the whole file"
    );
    let _ = std::fs::remove_dir_all(dir);
}

// ── CSV ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn csv_chunks_keep_rows_columns_and_the_header() {
    let processed = run(&pipeline(), &text_fixture("pessoas.csv")).await;
    check_chunks(&processed, "pessoas.csv");
    assert_eq!(processed.chunks.len(), 1);
    let chunk = &processed.chunks[0];
    assert_eq!(chunk.location, SourceLocation::csv(1, 2).unwrap());
    assert_eq!(
        chunk.metadata.columns,
        ["Nome", "Idade", "Cidade", "Profissão"]
    );
    assert!(chunk.text.starts_with(
        "Arquivo: pessoas.csv\nColunas: Nome, Idade, Cidade, Profissão\nLinhas 1–2\n\n"
    ));
    assert!(chunk.text.contains("Registro 1:\nNome: João\nIdade: 32"));
    assert!(chunk.text.contains("Registro 2:\nNome: Maria"));
    let dataset = processed.metadata.dataset.as_ref().expect("dataset");
    assert!(dataset.has_header);
    assert_eq!(dataset.row_count, Some(2));

    // A table with more rows than one chunk holds: contiguous, non-overlapping ranges.
    let dir = temp_dir("pipeline-csv");
    let path = dir.join("pedidos.csv");
    let mut csv = String::from("pedido,cliente,valor\n");
    for n in 1..=400 {
        csv.push_str(&format!("{n},Cliente {n},{n}0,50\n").replace(",50", ".50"));
    }
    std::fs::write(&path, csv).unwrap();
    let processed = run(&pipeline(), &path).await;
    check_chunks(&processed, "pedidos.csv");
    assert!(processed.chunks.len() > 3);
    let mut next = 1;
    for chunk in &processed.chunks {
        let SourceLocation::Csv { row_start, row_end } = chunk.location else {
            panic!("rows expected");
        };
        assert_eq!(row_start, next, "rows are contiguous");
        next = row_end + 1;
        assert_eq!(chunk.metadata.columns, ["pedido", "cliente", "valor"]);
    }
    assert_eq!(next, 401, "every row is in a chunk");
    let _ = std::fs::remove_dir_all(dir);
}

// ── EPUB ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn epub_chunks_keep_the_chapter_and_never_cross_chapters() {
    let path = epub_fixture("livro.epub");
    let pipeline = pipeline();
    let processed = run(&pipeline, &path).await;
    check_chunks(&processed, "livro.epub");

    // The words of each chapter, from the parsed document.
    let (parser, resolved) = pipeline
        .parsers
        .resolve(&DocumentSource::from_path(&path))
        .unwrap();
    let parsed = parser.parse(&resolved).await.unwrap();
    let mut chapters: std::collections::HashMap<u32, HashSet<String>> = Default::default();
    for block in parsed.blocks() {
        if let SourceLocation::Epub { chapter_index, .. } = &block.location {
            chapters
                .entry(*chapter_index)
                .or_default()
                .extend(words(&block.text));
        }
    }
    assert!(chapters.len() >= 3, "{}", chapters.len());

    let mut last_chapter = 0;
    for chunk in &processed.chunks {
        let SourceLocation::Epub {
            chapter_index,
            chapter_title,
            ..
        } = &chunk.location
        else {
            panic!("an EPUB chunk has a chapter: {:?}", chunk.location);
        };
        assert!(*chapter_index >= last_chapter, "chapters in reading order");
        last_chapter = *chapter_index;
        // The chapter title heads the section path ("Capítulo 3 > …").
        assert_eq!(
            chunk.section_path.first(),
            chapter_title.as_ref(),
            "{chunk:?}"
        );
        let known = &chapters[chapter_index];
        for word in words(&chunk.text) {
            assert!(
                known.contains(&word),
                "{word:?} is not in chapter {chapter_index}"
            );
        }
    }
    assert!(processed.metadata.title.is_some());
}

// ── Every format ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn every_chunk_of_every_format_is_located_and_only_pdf_is_previewable() {
    let pipeline = pipeline();
    let cases = [
        (fixture("report.pdf"), DocumentType::Pdf),
        (text_fixture("guia.md"), DocumentType::Markdown),
        (text_fixture("notas.txt"), DocumentType::Text),
        (text_fixture("vendas.csv"), DocumentType::Csv),
        (epub_fixture("livro.epub"), DocumentType::Epub),
    ];
    for (path, kind) in cases {
        let processed = run(&pipeline, &path).await;
        check_chunks(&processed, &path.file_name().unwrap().to_string_lossy());
        assert_eq!(processed.document_type, kind);
        for chunk in &processed.chunks {
            assert_eq!(chunk.location.previewable(), kind == DocumentType::Pdf);
            assert!(!chunk.location.label().is_empty());
        }
    }
}
