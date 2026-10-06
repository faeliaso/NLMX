//! The three parsers against the shared `DocumentParser` contract and the committed fixtures.

use std::path::{Path, PathBuf};

use nlmx_application::ports::{DocumentParser, DocumentSource};
use nlmx_domain::{
    document_type::DocumentType,
    parsed::{ContentKind, ParseError, ParseWarning},
    source::SourceLocation,
};
use nlmx_parser_text::{CsvDocumentParser, MarkdownDocumentParser, TextDocumentParser};
use nlmx_testing::{ParserSample, document_parser_contract};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

async fn contract(parser: &dyn DocumentParser, valid: &str, marker: &str, extension: &str) {
    document_parser_contract(
        parser,
        ParserSample {
            valid: &fixture(valid),
            marker,
            invalid: &fixture(&format!("invalido.{extension}")),
            missing: &fixture(&format!("nao-existe.{extension}")),
        },
    )
    .await;
}

#[tokio::test]
async fn text_parser_honours_the_contract() {
    contract(&TextDocumentParser, "notas.txt", "acentuação", "txt").await;
}

#[tokio::test]
async fn markdown_parser_honours_the_contract() {
    contract(&MarkdownDocumentParser, "guia.md", "Requisitos", "md").await;
}

#[tokio::test]
async fn csv_parser_honours_the_contract() {
    contract(&CsvDocumentParser, "vendas.csv", "Café", "csv").await;
}

#[tokio::test]
async fn the_notes_file_is_split_into_located_paragraphs() {
    let parsed = TextDocumentParser
        .parse(&DocumentSource::from_path(fixture("notas.txt")))
        .await
        .unwrap();
    let blocks: Vec<_> = parsed.blocks().collect();
    assert_eq!(blocks.len(), 4);
    assert_eq!(blocks[0].text, "Reunião de planejamento");
    assert_eq!(
        blocks[0].location,
        SourceLocation::text(0, "Reunião de planejamento".chars().count() as u32).unwrap()
    );
    assert!(blocks[2].text.contains("ç, ã, é e ü"));
}

#[tokio::test]
async fn a_windows_1252_file_is_read_as_utf8() {
    let parsed = TextDocumentParser
        .parse(&DocumentSource::from_path(fixture("latin1.txt")))
        .await
        .unwrap();
    let texts: Vec<_> = parsed.blocks().map(|b| b.text.as_str()).collect();
    assert_eq!(
        texts,
        ["Ação de coração", "Segundo parágrafo com acentuação."]
    );
    assert_eq!(parsed.warnings(), [ParseWarning::FallbackEncoding]);
}

#[tokio::test]
async fn a_whitespace_only_file_is_empty() {
    let result = TextDocumentParser
        .parse(&DocumentSource::from_path(fixture("vazio.txt")))
        .await;
    assert_eq!(result, Err(ParseError::Empty));
}

#[tokio::test]
async fn the_guide_keeps_its_structure() {
    let parsed = MarkdownDocumentParser
        .parse(&DocumentSource::from_path(fixture("guia.md")))
        .await
        .unwrap();
    assert_eq!(
        parsed.metadata().title.as_deref(),
        Some("Guia de Instalação")
    );
    assert_eq!(parsed.metadata().author.as_deref(), Some("Equipe NLMX"));
    assert_eq!(parsed.metadata().language.as_deref(), Some("pt-BR"));

    let paths: Vec<_> = parsed
        .sections()
        .iter()
        .map(|s| s.path.join(" > "))
        .collect();
    assert_eq!(
        paths,
        [
            "",
            "Guia de Instalação",
            "Guia de Instalação > Requisitos",
            "Guia de Instalação > Requisitos > Detalhes",
            "Guia de Instalação > Uso",
        ]
    );
    let kinds = |pred: fn(&ContentKind) -> bool| parsed.blocks().filter(|b| pred(&b.kind)).count();
    assert_eq!(kinds(|k| matches!(k, ContentKind::ListItem { .. })), 5);
    assert_eq!(kinds(|k| matches!(k, ContentKind::CodeBlock { .. })), 1);
    assert_eq!(kinds(|k| matches!(k, ContentKind::Table { .. })), 1);
    let code = parsed
        .blocks()
        .find(|b| matches!(b.kind, ContentKind::CodeBlock { .. }))
        .unwrap();
    assert_eq!(code.text, "make bootstrap\nmake dev");
    assert_eq!(
        code.kind,
        ContentKind::CodeBlock {
            language: Some("bash".into())
        }
    );
}

#[tokio::test]
async fn the_sales_file_keeps_columns_and_row_numbers() {
    let parsed = CsvDocumentParser
        .parse(&DocumentSource::from_path(fixture("vendas.csv")))
        .await
        .unwrap();
    let texts: Vec<_> = parsed.blocks().map(|b| b.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "Registro 1:\nproduto: Café\nquantidade: 3\npreço: 12,50\nobservação: Torra média embalagem de 500 g",
            "Registro 2:\nproduto: Chá verde\nquantidade: 10\npreço: 8,00",
            "Registro 3:\nproduto: Açúcar\nquantidade: 2\npreço: 4,75\nobservação: promoção",
        ]
    );
    assert!(parsed.warnings().is_empty());
    let rows: Vec<_> = parsed.blocks().map(|b| b.location.clone()).collect();
    assert_eq!(rows[2], SourceLocation::csv(3, 3).unwrap());
}

#[tokio::test]
async fn a_file_without_header_is_flagged() {
    let parsed = CsvDocumentParser
        .parse(&DocumentSource::from_path(fixture("sem-cabecalho.csv")))
        .await
        .unwrap();
    assert_eq!(parsed.warnings(), [ParseWarning::NoHeaderRow]);
    assert_eq!(parsed.blocks().count(), 3);
    assert!(
        parsed
            .blocks()
            .next()
            .unwrap()
            .text
            .starts_with("Registro 1:\ncoluna 1: Café")
    );
}

#[tokio::test]
async fn a_declared_format_other_than_the_parsers_is_refused() {
    let source = DocumentSource::of_type(fixture("notas.txt"), DocumentType::Csv);
    assert_eq!(
        TextDocumentParser.parse(&source).await,
        Err(ParseError::Unsupported)
    );
}

#[tokio::test]
async fn a_directory_is_not_a_document() {
    let dir = fixture("");
    assert_eq!(
        TextDocumentParser
            .parse(&DocumentSource::of_type(dir, DocumentType::Text))
            .await,
        Err(ParseError::NotFound)
    );
}
