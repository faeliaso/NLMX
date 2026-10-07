//! `EpubDocumentParser` on the committed fixtures and on books built in memory.

mod common;

use std::path::{Path, PathBuf};

use common::{Book, Chapter, livro, page, zip};
use nlmx_application::ports::{DocumentParser, DocumentSource};
use nlmx_domain::{
    document_type::DocumentType,
    parsed::{ContentKind, ParseError, ParseWarning, ParsedDocument},
    source::SourceLocation,
};
use nlmx_parser_epub::{EpubDocumentParser, Limits, parse_bytes, parse_bytes_with_limits};
use nlmx_testing::{ParserSample, document_parser_contract};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn paths(parsed: &ParsedDocument) -> Vec<Vec<String>> {
    parsed.sections().iter().map(|s| s.path.clone()).collect()
}

fn texts(parsed: &ParsedDocument) -> Vec<&str> {
    parsed.blocks().map(|b| b.text.as_str()).collect()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn the_parser_honours_the_contract() {
    document_parser_contract(
        &EpubDocumentParser::new(),
        ParserSample {
            valid: &fixture("livro.epub"),
            marker: "manhã fria",
            invalid: &fixture("corrompido.epub"),
            missing: &fixture("nao-existe.epub"),
        },
    )
    .await;
}

#[tokio::test]
async fn the_parser_reports_its_format_and_version() {
    let parser = EpubDocumentParser::new();
    assert_eq!(parser.document_type(), DocumentType::Epub);
    assert_eq!(parser.version(), nlmx_parser_epub::VERSION);
    assert!(parser.supports_mime("application/epub+zip"));
}

#[test]
fn the_fixture_is_the_one_the_generator_builds() {
    let committed = std::fs::read(fixture("livro.epub")).unwrap();
    assert_eq!(
        committed,
        livro().build(),
        "run the generate_fixtures example"
    );
}

#[test]
fn book_metadata_is_read() {
    let parsed = parse_bytes(&std::fs::read(fixture("livro.epub")).unwrap()).unwrap();
    let m = parsed.metadata();
    assert_eq!(m.title.as_deref(), Some("A Viagem de Inverno"));
    assert_eq!(m.author.as_deref(), Some("Ana Souza"));
    assert_eq!(m.language.as_deref(), Some("pt-BR"));
    assert_eq!(m.publisher.as_deref(), Some("Editora Exemplo"));
    assert_eq!(m.created_at.as_deref(), Some("2023-05-01"));
    assert_eq!(m.subject.as_deref(), Some("Viagens"));
    assert_eq!(m.source_version.as_deref(), Some("3.0"));
    assert_eq!(parsed.document_type(), DocumentType::Epub);
    assert!(parsed.pages().is_empty());
    assert!(parsed.warnings().is_empty());
}

#[test]
fn chapters_become_sections_with_their_titles_and_hierarchy() {
    let parsed = parse_bytes(&livro().build()).unwrap();
    assert_eq!(
        paths(&parsed),
        [
            strings(&["Partida"]),
            strings(&["Travessia"]),
            strings(&["Travessia", "O mar"]),
            strings(&["Travessia", "O mar", "As ondas"]),
            strings(&["Travessia", "O vento"]),
            strings(&["Chegada"]),
        ]
    );
    let levels: Vec<_> = parsed.sections().iter().map(|s| s.level).collect();
    assert_eq!(levels, [1, 1, 2, 3, 2, 1]);
    // Each section starts with its heading block, at the section's level.
    for section in parsed.sections() {
        let first = &section.blocks[0];
        assert_eq!(
            first.kind,
            ContentKind::Heading {
                level: section.level
            }
        );
        assert_eq!(Some(first.text.as_str()), section.title.as_deref());
        assert_eq!(first.location, section.location);
    }
}

#[test]
fn locations_name_the_chapter_and_the_section() {
    let parsed = parse_bytes(&livro().build()).unwrap();
    let section = |path: &[&str]| {
        parsed
            .sections()
            .iter()
            .find(|s| s.path == strings(path))
            .unwrap()
    };
    let ondas = section(&["Travessia", "O mar", "As ondas"]);
    assert_eq!(
        ondas.blocks[1].location,
        SourceLocation::epub(2, Some("Travessia".into()), Some("As ondas".into())).unwrap()
    );
    // Before the first inner heading, a block belongs to the chapter itself.
    let travessia = section(&["Travessia"]);
    assert_eq!(
        travessia.blocks[1].location,
        SourceLocation::epub(2, Some("Travessia".into()), None).unwrap()
    );
    assert_eq!(travessia.blocks[1].text, "O mar estava calmo.");
    // The title of a chapter without a heading comes from the table of contents.
    let chegada = section(&["Chegada"]);
    assert_eq!(chegada.blocks[0].text, "Chegada");
    assert_eq!(
        chegada.blocks[1].location,
        SourceLocation::epub(3, Some("Chegada".into()), None).unwrap()
    );
    assert_eq!(chegada.blocks[1].location.label(), "cap. 3 — Chegada");
}

#[test]
fn lists_code_and_tables_keep_their_structure() {
    let parsed = parse_bytes(&livro().build()).unwrap();
    let kinds: Vec<_> = parsed.blocks().map(|b| &b.kind).collect();
    assert!(kinds.contains(&&ContentKind::ListItem {
        ordered: false,
        depth: 0
    }));
    assert!(kinds.contains(&&ContentKind::CodeBlock {
        language: Some("python".into())
    }));
    let table = parsed
        .blocks()
        .find(|b| matches!(b.kind, ContentKind::Table { .. }))
        .unwrap();
    assert_eq!(
        table.kind,
        ContentKind::Table {
            header: strings(&["Porto", "Dias"]),
            rows: vec![strings(&["Santos", "12"])],
        }
    );
    assert_eq!(table.text, "Porto | Dias\nSantos | 12");
    let code = parsed
        .blocks()
        .find(|b| matches!(b.kind, ContentKind::CodeBlock { .. }))
        .unwrap();
    assert_eq!(code.text, "print(\"olá, mar\")");
}

#[test]
fn entities_and_accents_are_decoded() {
    let parsed = parse_bytes(&livro().build()).unwrap();
    let all = texts(&parsed);
    assert!(all.contains(&"A viagem de inverno começou numa manhã fria, com neve nas montanhas."));
    assert!(all.contains(&"Levamos café & pão quente na mochila."));
}

#[test]
fn titles_come_from_the_ncx_when_there_is_no_nav() {
    let mut book = Book::new(vec![
        Chapter::new("a.xhtml", "<p>Texto A</p>"),
        Chapter::new("b.xhtml", "<p>Texto B</p>"),
    ]);
    book.ncx = Some(vec![
        ("a.xhtml".into(), "Primeiro".into()),
        ("b.xhtml#x".into(), "Segundo & Último".into()),
    ]);
    let parsed = parse_bytes(&book.build()).unwrap();
    assert_eq!(
        paths(&parsed),
        [strings(&["Primeiro"]), strings(&["Segundo & Último"])]
    );
}

#[test]
fn the_nav_wins_over_the_ncx_and_the_ncx_fills_the_gaps() {
    let mut book = Book::new(vec![
        Chapter::new("a.xhtml", "<p>A</p>"),
        Chapter::new("b.xhtml", "<p>B</p>"),
    ]);
    book.nav = Some(vec![("a.xhtml".into(), "Do nav".into())]);
    book.ncx = Some(vec![
        ("a.xhtml".into(), "Do ncx".into()),
        ("b.xhtml".into(), "Só no ncx".into()),
    ]);
    let parsed = parse_bytes(&book.build()).unwrap();
    assert_eq!(
        paths(&parsed),
        [strings(&["Do nav"]), strings(&["Só no ncx"])]
    );
}

#[test]
fn without_a_toc_the_first_heading_names_the_chapter_and_the_fallback_is_numbered() {
    let book = Book::new(vec![
        Chapter::new("a.xhtml", "<h2>Abertura</h2><p>Texto.</p>"),
        Chapter::new("b.xhtml", "<p>Sem título nenhum.</p>"),
    ]);
    let parsed = parse_bytes(&book.build()).unwrap();
    assert_eq!(
        paths(&parsed),
        [strings(&["Abertura"]), strings(&["Capítulo 2"])]
    );
    // The heading that names the chapter is not repeated.
    assert_eq!(parsed.sections()[0].blocks.len(), 2);
}

#[test]
fn a_chapter_that_repeats_its_nav_title_in_its_heading_is_not_duplicated() {
    let mut book = Book::new(vec![Chapter::new(
        "a.xhtml",
        "<h1>Partida</h1><p>Texto.</p>",
    )]);
    book.nav = Some(vec![("a.xhtml".into(), "Partida".into())]);
    let parsed = parse_bytes(&book.build()).unwrap();
    assert_eq!(parsed.sections().len(), 1);
    assert_eq!(texts(&parsed), ["Partida", "Texto."]);
}

#[test]
fn a_different_first_heading_nests_under_the_toc_title() {
    let mut book = Book::new(vec![Chapter::new(
        "a.xhtml",
        "<h1>Outro título</h1><p>Texto.</p><h1>Outro irmão</h1><p>Mais.</p>",
    )]);
    book.nav = Some(vec![("a.xhtml".into(), "Capítulo do sumário".into())]);
    let parsed = parse_bytes(&book.build()).unwrap();
    assert_eq!(
        paths(&parsed),
        [
            strings(&["Capítulo do sumário"]),
            strings(&["Capítulo do sumário", "Outro título"]),
            strings(&["Capítulo do sumário", "Outro irmão"]),
        ]
    );
}

#[test]
fn files_in_subdirectories_with_escaped_names_resolve() {
    // The zip entry is `OEBPS/text/capítulo 1.xhtml`; the manifest says it escaped.
    let mut chapter = Chapter::new("text/capítulo 1.xhtml", "<p>Achado.</p>");
    chapter.href = Some("text/cap%C3%ADtulo%201.xhtml".into());
    let parsed = parse_bytes(&Book::new(vec![chapter]).build()).unwrap();
    assert_eq!(texts(&parsed), ["Capítulo 1", "Achado."]);
}

#[test]
fn a_non_linear_item_is_kept_in_place() {
    let mut book = Book::new(vec![
        Chapter::new("a.xhtml", "<p>Principal.</p>"),
        Chapter::new("notas.xhtml", "<p>Notas de rodapé.</p>"),
    ]);
    book.chapters[1].linear = false;
    let parsed = parse_bytes(&book.build()).unwrap();
    assert_eq!(texts(&parsed).last(), Some(&"Notas de rodapé."));
}

#[test]
fn a_malformed_chapter_is_skipped_with_a_warning() {
    let book = Book::new(vec![
        Chapter::new("a.xhtml", "<p>Primeiro.</p>"),
        Chapter::raw(
            "b.xhtml",
            b"<html><body><p>quebrado \xff\xfe</p></body></html>",
        ),
        Chapter::new("c.xhtml", "<p>Terceiro.</p>"),
    ]);
    let parsed = parse_bytes(&book.build()).unwrap();
    assert_eq!(parsed.warnings(), [ParseWarning::SkippedUnit { index: 2 }]);
    let chapters: Vec<_> = parsed
        .blocks()
        .filter_map(|b| match &b.location {
            SourceLocation::Epub { chapter_index, .. } => Some(*chapter_index),
            _ => None,
        })
        .collect();
    assert!(chapters.contains(&1) && chapters.contains(&3) && !chapters.contains(&2));
}

#[test]
fn a_chapter_missing_from_the_zip_is_skipped_with_a_warning() {
    let mut book = Book::new(vec![Chapter::new("a.xhtml", "<p>Presente.</p>")]);
    book.missing_files = vec!["sumiu.xhtml".into()];
    let parsed = parse_bytes(&book.build()).unwrap();
    assert_eq!(parsed.warnings(), [ParseWarning::SkippedUnit { index: 2 }]);
    assert!(texts(&parsed).contains(&"Presente."));
}

#[test]
fn a_book_where_every_chapter_is_unreadable_is_invalid() {
    let book = Book::new(vec![
        Chapter::raw("a.xhtml", b"<p>\xff</p>"),
        Chapter::raw("b.xhtml", b"<p>\xfe</p>"),
    ]);
    assert_eq!(
        parse_bytes(&book.build()),
        Err(ParseError::Invalid(DocumentType::Epub))
    );
}

#[test]
fn a_book_without_text_is_empty() {
    let bytes = std::fs::read(fixture("sem-texto.epub")).unwrap();
    assert_eq!(parse_bytes(&bytes), Err(ParseError::Empty));
}

#[test]
fn drm_is_refused() {
    let bytes = std::fs::read(fixture("drm.epub")).unwrap();
    assert_eq!(parse_bytes(&bytes), Err(ParseError::Drm));
}

#[test]
fn font_obfuscation_is_not_drm() {
    let mut book = livro();
    book.encryption = vec![
        "http://www.idpf.org/2008/embedding".into(),
        "http://ns.adobe.com/pdf/enc#RC".into(),
    ];
    assert!(parse_bytes(&book.build()).is_ok());
    // One real algorithm among them is enough to refuse the book.
    book.encryption
        .push("http://www.w3.org/2001/04/xmlenc#aes128-cbc".into());
    assert_eq!(parse_bytes(&book.build()), Err(ParseError::Drm));
}

#[test]
fn files_that_are_not_books_are_invalid() {
    let invalid = ParseError::Invalid(DocumentType::Epub);
    assert_eq!(parse_bytes(b"isto nao e um zip"), Err(invalid.clone()));
    assert_eq!(parse_bytes(b""), Err(invalid.clone()));
    // A zip without container.xml.
    assert_eq!(
        parse_bytes(&zip(&[
            ("mimetype", b"application/epub+zip"),
            ("a.txt", b"x")
        ])),
        Err(invalid.clone())
    );
    // A container that points to a package that is not there.
    let container =
        br#"<container><rootfiles><rootfile full-path="x/content.opf"/></rootfiles></container>"#;
    assert_eq!(
        parse_bytes(&zip(&[("META-INF/container.xml", container)])),
        Err(invalid.clone())
    );
    // A package with an empty spine.
    let opf = b"<package><metadata/><manifest/><spine/></package>";
    assert_eq!(
        parse_bytes(&zip(&[
            ("META-INF/container.xml", container),
            ("x/content.opf", opf)
        ])),
        Err(invalid)
    );
    // The truncated fixture.
    let damaged = std::fs::read(fixture("corrompido.epub")).unwrap();
    assert_eq!(
        parse_bytes(&damaged),
        Err(ParseError::Invalid(DocumentType::Epub))
    );
}

#[test]
fn a_huge_entry_is_refused() {
    let big = vec![b' '; 2 * 1024 * 1024];
    let mut content = page("<p>Texto</p>");
    content.extend_from_slice(&big);
    let book = Book::new(vec![Chapter::raw("a.xhtml", &content)]);
    let limits = Limits {
        max_entry_bytes: 1024 * 1024,
        ..Limits::default()
    };
    let bytes = book.build();
    // It compresses to a few kilobytes: the limit is on the uncompressed size.
    assert!(bytes.len() < 10_000);
    assert_eq!(
        parse_bytes_with_limits(&bytes, limits),
        Err(ParseError::TooLarge)
    );
    assert!(parse_bytes(&bytes).is_ok());
}

#[test]
fn too_much_uncompressed_data_in_total_is_refused() {
    let chunk = {
        let mut c = page("<p>Texto</p>");
        c.extend_from_slice(&vec![b' '; 400 * 1024]);
        c
    };
    let chapters = (0..4)
        .map(|i| Chapter::raw(&format!("c{i}.xhtml"), &chunk))
        .collect();
    let bytes = Book::new(chapters).build();
    let limits = Limits {
        max_total_bytes: 1024 * 1024,
        ..Limits::default()
    };
    assert_eq!(
        parse_bytes_with_limits(&bytes, limits),
        Err(ParseError::TooLarge)
    );
}

#[test]
fn too_many_entries_are_refused() {
    let mut book = livro();
    book.extra_files = (0..30)
        .map(|i| (format!("img/{i}.png"), vec![0u8; 4]))
        .collect();
    let limits = Limits {
        max_entries: 20,
        ..Limits::default()
    };
    assert_eq!(
        parse_bytes_with_limits(&book.build(), limits),
        Err(ParseError::TooLarge)
    );
}

#[tokio::test]
async fn a_file_over_the_limit_is_refused_without_reading_it() {
    let parser = EpubDocumentParser::with_limits(Limits {
        max_total_bytes: 100,
        ..Limits::default()
    });
    let result = parser
        .parse(&DocumentSource::from_path(fixture("livro.epub")))
        .await;
    assert_eq!(result, Err(ParseError::TooLarge));
}

#[tokio::test]
async fn another_declared_format_is_unsupported() {
    let source = DocumentSource::of_type(fixture("livro.epub"), DocumentType::Pdf);
    assert_eq!(
        EpubDocumentParser::new().parse(&source).await,
        Err(ParseError::Unsupported)
    );
}

#[tokio::test]
async fn errors_do_not_leak_the_content_or_the_path() {
    let parser = EpubDocumentParser::new();
    for name in ["corrompido.epub", "drm.epub", "nao-existe.epub"] {
        let path = fixture(name);
        let error = parser
            .parse(&DocumentSource::from_path(&path))
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains(&*path.to_string_lossy()), "{error}");
        assert!(!error.contains("Viagem"), "{error}");
    }
}
