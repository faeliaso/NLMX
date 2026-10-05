//! `DocxDocumentParser` on one small fixture per scenario: plain text, headings, lists, tables,
//! hyperlinks, several sections and metadata, plus encodings and broken files.

mod common;

use std::path::{Path, PathBuf};

use common::{
    contrato, core, docx, docx_from_document, docx_with_core, fixtures, li, merged_row, p, para,
    run, table, zip,
};
use nlmx_application::ports::{DocumentParser, DocumentSource};
use nlmx_domain::{
    document_type::DocumentType,
    parsed::{ContentKind, ParseError, ParsedDocument},
    source::SourceLocation,
};
use nlmx_parser_office::{DocxDocumentParser, parse_docx};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn parse_fixture(name: &str) -> ParsedDocument {
    parse_docx(&std::fs::read(fixture(name)).unwrap()).unwrap()
}

fn texts(parsed: &ParsedDocument) -> Vec<&str> {
    parsed.blocks().map(|b| b.text.as_str()).collect()
}

fn paths(parsed: &ParsedDocument) -> Vec<Vec<String>> {
    parsed.sections().iter().map(|s| s.path.clone()).collect()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn every_docx_fixture_is_the_one_the_generator_builds() {
    assert_eq!(std::fs::read(fixture("contrato.docx")).unwrap(), contrato());
    for (name, bytes) in fixtures() {
        assert_eq!(
            std::fs::read(fixture(name)).unwrap(),
            bytes,
            "{name}: run the generate_fixtures example"
        );
    }
}

#[tokio::test]
async fn every_docx_fixture_goes_through_the_parser_port() {
    for (name, _) in fixtures() {
        let parsed = DocxDocumentParser::new()
            .parse(&DocumentSource::from_path(fixture(name)))
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(parsed.validate(), Ok(()), "{name}");
        assert!(parsed.has_text(), "{name}");
    }
}

#[test]
fn a_simple_text_keeps_its_paragraphs_in_order_as_utf8() {
    let parsed = parse_fixture("simples.docx");
    assert_eq!(
        texts(&parsed),
        [
            "Primeiro parágrafo, sem estrutura.",
            "Segundo parágrafo com acentuação: ação, coração."
        ]
    );
    assert_eq!(parsed.sections().len(), 1);
    assert!(parsed.sections()[0].path.is_empty());
}

#[test]
fn headings_keep_their_hierarchy() {
    let parsed = parse_fixture("titulos.docx");
    assert_eq!(
        paths(&parsed),
        vec![
            strings(&["Arquitetura"]),
            strings(&["Arquitetura", "Backend"]),
            strings(&["Arquitetura", "Backend", "Autenticação"]),
            strings(&["Arquitetura", "Frontend"]),
        ]
    );
    let auth = parsed
        .blocks()
        .find(|b| b.text.starts_with("Tokens"))
        .unwrap();
    assert_eq!(
        auth.location,
        SourceLocation::docx(
            strings(&["Arquitetura", "Backend", "Autenticação"]),
            Some((6, 6))
        )
        .unwrap()
    );
    assert_eq!(
        auth.location.label(),
        "Arquitetura › Backend › Autenticação"
    );
}

#[test]
fn a_skipped_heading_level_does_not_invent_a_parent() {
    let body = format!(
        "{}{}{}",
        p(Some("Ttulo1"), "Raiz"),
        p(Some("Ttulo3"), "Fundo"),
        p(None, "texto")
    );
    let parsed = parse_docx(&docx(&body)).unwrap();
    assert_eq!(
        paths(&parsed),
        vec![strings(&["Raiz"]), strings(&["Raiz", "Fundo"])]
    );
}

#[test]
fn lists_keep_their_items_kind_and_text() {
    let parsed = parse_fixture("listas.docx");
    let items: Vec<(&str, &ContentKind)> = parsed
        .blocks()
        .map(|b| (b.text.as_str(), &b.kind))
        .collect();
    assert_eq!(items[0], ("Requisitos:", &ContentKind::Paragraph));
    assert_eq!(
        items[1],
        (
            "Autenticação",
            &ContentKind::ListItem {
                ordered: false,
                depth: 0
            }
        )
    );
    assert_eq!(items[2].0, "Autorização");
    assert_eq!(
        items[4],
        (
            "Instalar",
            &ContentKind::ListItem {
                ordered: true,
                depth: 0
            }
        )
    );
    assert_eq!(items[5].0, "Configurar");
}

#[test]
fn a_table_becomes_one_record_per_row_that_says_it_is_a_table() {
    let parsed = parse_fixture("tabela.docx");
    let rows: Vec<_> = parsed
        .blocks()
        .filter(|b| matches!(b.kind, ContentKind::Record { .. }))
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].text,
        "Tabela 1: Nome | Cidade | Cargo\nNome: João\nCidade: Fortaleza\nCargo: Engenheiro"
    );
    assert_eq!(
        rows[1].text,
        "Tabela 1: Nome | Cidade | Cargo\nNome: Maria\nCidade: Recife\nCargo: Analista"
    );
    let ContentKind::Record { fields } = &rows[0].kind else {
        unreachable!()
    };
    assert_eq!(fields[2].name, "Cargo");
    assert_eq!(fields[2].value, "Engenheiro");
    // Each row is located on its own, under the heading that precedes the table.
    assert_eq!(
        rows[1].location,
        SourceLocation::docx_table(strings(&["Equipe"]), Some((3, 3)), Some(1)).unwrap()
    );
}

#[test]
fn a_header_is_guessed_when_the_file_does_not_declare_one() {
    let body = table(&[&["Item", "Preço"], &["Caneta", "2"], &["Lápis", "1"]]);
    let parsed = parse_docx(&docx(&body)).unwrap();
    assert_eq!(
        texts(&parsed),
        [
            "Tabela 1: Item | Preço\nItem: Caneta\nPreço: 2",
            "Tabela 1: Item | Preço\nItem: Lápis\nPreço: 1"
        ]
    );
}

#[test]
fn a_table_without_a_trustworthy_header_is_kept_as_lines() {
    // Repeated first-row cells: not a header.
    let parsed = parse_docx(&docx(&table(&[&["x", "x"], &["a", "b"]]))).unwrap();
    assert_eq!(texts(&parsed), ["x | x\na | b"]);
    assert!(matches!(
        parsed.blocks().next().unwrap().kind,
        ContentKind::Table { .. }
    ));
    // A single row: nothing to name.
    let single = parse_docx(&docx(&table(&[&["só", "uma"]]))).unwrap();
    assert_eq!(texts(&single), ["só | uma"]);
}

#[test]
fn merged_cells_keep_their_text_and_empty_rows_are_skipped() {
    let body = format!(
        "<w:tbl><w:tr><w:trPr><w:tblHeader/></w:trPr><w:tc>{}</w:tc><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr>{}{}</w:tbl>",
        p(None, "Nome"),
        p(None, "Cidade"),
        p(None, "Cargo"),
        merged_row("Ana (Fortaleza)", 2, &["Gerente"]),
        "<w:tr><w:tc><w:p/></w:tc><w:tc><w:p/></w:tc><w:tc><w:p/></w:tc></w:tr>",
    );
    let parsed = parse_docx(&docx(&body)).unwrap();
    assert_eq!(
        texts(&parsed),
        ["Tabela 1: Nome | Cidade | Cargo\nNome: Ana (Fortaleza)\nCargo: Gerente"]
    );
}

#[test]
fn a_hyperlink_keeps_its_text_in_the_paragraph() {
    let parsed = parse_fixture("links.docx");
    assert_eq!(
        texts(&parsed),
        ["Veja a documentação oficial para detalhes."]
    );
}

#[test]
fn field_codes_are_not_text_but_their_result_is() {
    let runs = [
        run("Acesse "),
        "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r>".to_string(),
        "<w:r><w:instrText xml:space=\"preserve\"> HYPERLINK \"https://exemplo.com\" </w:instrText></w:r>"
            .to_string(),
        "<w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>".to_string(),
        run("o site"),
        "<w:r><w:fldChar w:fldCharType=\"end\"/></w:r>".to_string(),
        "<w:fldSimple w:instr=\"PAGE\"><w:r><w:t>3</w:t></w:r></w:fldSimple>".to_string(),
    ]
    .concat();
    let parsed = parse_docx(&docx(&para(&runs))).unwrap();
    assert_eq!(texts(&parsed), ["Acesse o site3"]);
}

#[test]
fn word_sections_do_not_break_the_reading_order() {
    let parsed = parse_fixture("secoes.docx");
    assert_eq!(
        texts(&parsed),
        [
            "Parte um",
            "Fim da primeira seção.",
            "Parte dois",
            "Conteúdo da segunda seção.",
            "Fim da segunda seção.",
            "Parte três",
            "Conteúdo final.",
        ]
    );
    assert_eq!(
        paths(&parsed),
        vec![
            strings(&["Parte um"]),
            strings(&["Parte dois"]),
            strings(&["Parte três"]),
        ]
    );
}

#[test]
fn the_basic_metadata_is_extracted() {
    let m = parse_fixture("metadados.docx").metadata().clone();
    assert_eq!(m.title.as_deref(), Some("Manual do Sistema"));
    assert_eq!(m.author.as_deref(), Some("Ana Souza"));
    assert_eq!(m.subject.as_deref(), Some("Operação"));
    assert_eq!(m.created_at.as_deref(), Some("2024-03-01T10:00:00Z"));
    assert_eq!(m.modified_at.as_deref(), Some("2024-05-20T16:30:00Z"));
}

#[test]
fn metadata_is_optional_and_never_required() {
    let body = p(None, "Texto.");
    for core_part in [
        None,
        Some(core(None, None, None, None, None)),
        Some("<not xml".to_string()),
    ] {
        let parsed = parse_docx(&docx_with_core(&body, core_part.as_deref())).unwrap();
        assert_eq!(texts(&parsed), ["Texto."]);
        assert_eq!(parsed.metadata().title, None);
    }
}

#[test]
fn utf16_and_bom_documents_are_normalized_to_utf8() {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-16\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{}</w:body></w:document>",
        p(None, "Ação em UTF-16.")
    );
    let mut le = vec![0xFF, 0xFE];
    le.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
    let mut bom = vec![0xEF, 0xBB, 0xBF];
    bom.extend(xml.as_bytes());
    for part in [le, bom] {
        let parsed = parse_docx(&docx_from_document(&part, None)).unwrap();
        assert_eq!(texts(&parsed), ["Ação em UTF-16."]);
    }
    // A UTF-16 part that is cut in half is an invalid document, not a crash.
    assert_eq!(
        parse_docx(&docx_from_document(&[0xFF, 0xFE, 0x41], None)),
        Err(ParseError::Invalid(DocumentType::Docx))
    );
}

#[test]
fn broken_documents_end_in_controlled_errors() {
    // Cut in the middle of the body: whatever the reader makes of it, it does not panic.
    let _ = parse_docx(&docx_from_document(
        b"<w:document><w:body><w:p><w:r><w:t>x</w:t></w:r></w:p",
        None,
    ));
    assert_eq!(
        parse_docx(&zip(&[("word/document.xml", b"<w:document/>")])),
        Err(ParseError::Empty)
    );
    assert_eq!(
        parse_docx(&zip(&[("_rels/.rels", b"<Relationships/>")])),
        Err(ParseError::Invalid(DocumentType::Docx)),
        "no document part"
    );
    assert_eq!(
        parse_docx(b""),
        Err(ParseError::Invalid(DocumentType::Docx))
    );
}

#[test]
fn list_items_keep_depth_in_the_numbering() {
    let body = format!("{}{}", li(2, 0, "um"), li(1, 1, "sub"));
    let parsed = parse_docx(&docx(&body)).unwrap();
    let kinds: Vec<_> = parsed.blocks().map(|b| b.kind.clone()).collect();
    assert_eq!(
        kinds[1],
        ContentKind::ListItem {
            ordered: false,
            depth: 1
        }
    );
}

// ---------------------------------------------------------------- the manual

fn manual_blocks_of(
    parsed: &ParsedDocument,
    path: &[&str],
) -> Vec<(ContentKind, String, SourceLocation)> {
    let wanted: Vec<String> = path.iter().map(|s| s.to_string()).collect();
    parsed
        .sections()
        .iter()
        .filter(|s| s.path == wanted)
        .flat_map(|s| s.blocks.iter())
        .map(|b| (b.kind.clone(), b.text.clone(), b.location.clone()))
        .collect()
}

#[test]
fn the_manual_keeps_the_section_tree_for_every_block() {
    let parsed = parse_fixture("manual.docx");
    assert_eq!(
        paths(&parsed),
        vec![
            strings(&["Introdução"]),
            strings(&["Arquitetura"]),
            strings(&["Arquitetura", "Backend"]),
            strings(&["Arquitetura", "Frontend"]),
            strings(&["Deploy"]),
        ]
    );
    for (kind, _, location) in manual_blocks_of(&parsed, &["Arquitetura", "Backend"]) {
        let SourceLocation::Docx { heading_path, .. } = &location else {
            panic!("a DOCX location");
        };
        assert_eq!(
            heading_path,
            &strings(&["Arquitetura", "Backend"]),
            "{kind:?}"
        );
        assert_eq!(location.label(), "Arquitetura › Backend");
        assert_eq!(location.page_range(), None, "a DOCX has no pages");
    }
}

#[test]
fn list_items_belong_to_the_section_they_are_in() {
    let parsed = parse_fixture("manual.docx");
    let items: Vec<_> = manual_blocks_of(&parsed, &["Arquitetura", "Backend"])
        .into_iter()
        .filter(|(kind, ..)| matches!(kind, ContentKind::ListItem { .. }))
        .collect();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].1, "Autenticação por token");
}

#[test]
fn tables_are_numbered_in_document_order_and_a_caption_names_them() {
    let parsed = parse_fixture("manual.docx");
    let first = manual_blocks_of(&parsed, &["Arquitetura", "Frontend"]);
    let rows: Vec<_> = first
        .iter()
        .filter(|(kind, ..)| matches!(kind, ContentKind::Record { .. }))
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].1,
        "Tabela 1 — Telas principais: Tela | Responsável | Estado\nTela: Login\nResponsável: Marina\nEstado: Pronta"
    );
    assert_eq!(rows[0].2.label(), "Arquitetura › Frontend, tabela 1");

    let second = manual_blocks_of(&parsed, &["Deploy"]);
    let row = second
        .iter()
        .find(|(kind, ..)| matches!(kind, ContentKind::Record { .. }))
        .unwrap();
    assert_eq!(
        row.1, "Tabela 2: Ambiente | Região\nAmbiente: Produção\nRegião: sa-east-1",
        "no caption: just the index"
    );
    assert_eq!(row.2.label(), "Deploy, tabela 2");
    assert!(
        second
            .iter()
            .any(|(kind, text, loc)| *kind == ContentKind::Paragraph
                && text.starts_with("Entrega")
                && matches!(loc, SourceLocation::Docx { table: None, .. })),
        "prose after a table is not table text"
    );
}

#[test]
fn caption_titles_drop_the_number_they_repeat() {
    for (caption, expected) in [
        ("Tabela 2 – Equipe", "Equipe"),
        ("Table 10: Team", "Team"),
        ("Equipe do projeto", "Equipe do projeto"),
    ] {
        let body = format!(
            "{}{}",
            p(Some("Legenda"), caption),
            common::table_header(&[&["A", "B"], &["1", "2"]])
        );
        let parsed = parse_docx(&docx(&body)).unwrap();
        let record = parsed
            .blocks()
            .find(|b| matches!(b.kind, ContentKind::Record { .. }))
            .unwrap();
        assert!(
            record
                .text
                .starts_with(&format!("Tabela 1 — {expected}: A | B")),
            "{}",
            record.text
        );
    }
    // A caption with nothing but the number leaves just the index.
    let body = format!(
        "{}{}",
        p(Some("Legenda"), "Tabela 3"),
        common::table_header(&[&["A", "B"], &["1", "2"]])
    );
    let parsed = parse_docx(&docx(&body)).unwrap();
    assert!(
        parsed
            .blocks()
            .any(|b| b.text.starts_with("Tabela 1: A | B"))
    );
}

#[test]
fn a_caption_far_from_the_table_is_not_its_caption() {
    let body = format!(
        "{}{}{}",
        p(Some("Legenda"), "Figura 1 – Diagrama"),
        p(None, "Texto no meio."),
        common::table_header(&[&["A", "B"], &["1", "2"]])
    );
    let parsed = parse_docx(&docx(&body)).unwrap();
    assert!(
        parsed
            .blocks()
            .any(|b| b.text.starts_with("Tabela 1: A | B"))
    );
}
