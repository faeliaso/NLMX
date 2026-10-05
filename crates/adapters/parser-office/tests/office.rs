//! `DocxDocumentParser` and `XlsxDocumentParser` on the committed fixtures and on files built
//! in memory.

mod common;

use std::path::{Path, PathBuf};

use common::{Sheet, cell, contrato, docx, li, p, styled, table, vendas, xlsx, zip};
use nlmx_application::ports::{DocumentParser, DocumentSource};
use nlmx_domain::{
    document_type::DocumentType,
    parsed::{ContentKind, ParseError, ParseWarning, ParsedDocument},
    source::SourceLocation,
};
use nlmx_parser_office::{
    DocxDocumentParser, Limits, XlsxDocumentParser, parse_docx, parse_docx_with_limits, parse_xlsx,
    parse_xlsx_with_limits,
};
use nlmx_testing::{ParserSample, document_parser_contract};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
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

#[tokio::test]
async fn the_docx_parser_honours_the_contract() {
    document_parser_contract(
        &DocxDocumentParser::new(),
        ParserSample {
            valid: &fixture("contrato.docx"),
            marker: "manutenção predial",
            invalid: &fixture("corrompido.docx"),
            missing: &fixture("nao-existe.docx"),
        },
    )
    .await;
}

#[tokio::test]
async fn the_xlsx_parser_honours_the_contract() {
    document_parser_contract(
        &XlsxDocumentParser::new(),
        ParserSample {
            valid: &fixture("vendas.xlsx"),
            marker: "Cadeira",
            invalid: &fixture("corrompido.xlsx"),
            missing: &fixture("nao-existe.xlsx"),
        },
    )
    .await;
}

#[test]
fn the_parsers_report_their_format_and_version() {
    assert_eq!(
        DocxDocumentParser::new().document_type(),
        DocumentType::Docx
    );
    assert_eq!(
        XlsxDocumentParser::new().document_type(),
        DocumentType::Xlsx
    );
    assert_eq!(
        DocxDocumentParser::new().version(),
        nlmx_parser_office::DOCX_VERSION
    );
    assert_eq!(
        XlsxDocumentParser::new().version(),
        nlmx_parser_office::XLSX_VERSION
    );
}

#[test]
fn the_fixtures_are_the_ones_the_generator_builds() {
    assert_eq!(std::fs::read(fixture("contrato.docx")).unwrap(), contrato());
    assert_eq!(std::fs::read(fixture("vendas.xlsx")).unwrap(), vendas());
}

// ---------------------------------------------------------------- DOCX

#[test]
fn a_docx_is_read_as_sections_of_typed_blocks() {
    let parsed = parse_docx(&contrato()).unwrap();
    assert_eq!(parsed.document_type(), DocumentType::Docx);
    assert_eq!(
        paths(&parsed),
        vec![
            strings(&[]),
            strings(&["Objeto"]),
            strings(&["Objeto", "Prazos"]),
            strings(&["Disposições gerais"]),
            strings(&["Disposições gerais", "Foro"]),
        ]
    );
    assert_eq!(
        texts(&parsed),
        [
            "Minuta preliminar sujeita a revisão.",
            "Objeto",
            "O presente contrato trata da prestação de serviços de manutenção predial.",
            "Prazos",
            "O prazo de vigência é de vinte e quatro meses & renovável.",
            "Aviso prévio de trinta dias",
            "Multa de dez por cento",
            "Tabela 1: Parcela | Valor\nParcela: Entrada\nValor: R$ 1.000",
            "Tabela 1: Parcela | Valor\nParcela: Final\nValor: R$ 2.000",
            "Disposições gerais",
            "Foro",
            "Fica eleito o foro da comarca de Fortaleza.",
        ]
    );
    let kinds: Vec<&ContentKind> = parsed.blocks().map(|b| &b.kind).collect();
    assert_eq!(kinds[1], &ContentKind::Heading { level: 1 });
    assert_eq!(kinds[3], &ContentKind::Heading { level: 2 });
    assert_eq!(
        kinds[5],
        &ContentKind::ListItem {
            ordered: false,
            depth: 0
        }
    );
    assert_eq!(
        kinds[6],
        &ContentKind::ListItem {
            ordered: true,
            depth: 0
        }
    );
    // A table with a header reads as one record per row.
    assert!(matches!(kinds[7], ContentKind::Record { fields } if fields.len() == 2));
    assert!(matches!(kinds[8], ContentKind::Record { .. }));
    // A custom style based on "heading 2" is a heading too.
    assert_eq!(kinds[10], &ContentKind::Heading { level: 2 });
}

#[test]
fn docx_blocks_are_located_by_headings_and_paragraph() {
    let parsed = parse_docx(&contrato()).unwrap();
    let located: Vec<_> = parsed.blocks().map(|b| b.location.clone()).collect();
    assert_eq!(
        located[0],
        SourceLocation::docx(vec![], Some((1, 1))).unwrap()
    );
    assert_eq!(
        located[2],
        SourceLocation::docx(strings(&["Objeto"]), Some((3, 3))).unwrap()
    );
    assert_eq!(
        located[4],
        SourceLocation::docx(strings(&["Objeto", "Prazos"]), Some((5, 5))).unwrap()
    );
    // A level-1 heading closes the level-2 one before it.
    assert_eq!(
        located[9],
        SourceLocation::docx(strings(&["Disposições gerais"]), Some((10, 10))).unwrap()
    );
    assert!(
        located
            .iter()
            .all(|l| l.document_type() == DocumentType::Docx)
    );
}

#[test]
fn docx_metadata_comes_from_the_core_properties() {
    let m = parse_docx(&contrato()).unwrap().metadata().clone();
    assert_eq!(m.title.as_deref(), Some("Contrato de Serviços"));
    assert_eq!(m.author.as_deref(), Some("Ana Souza"));
    assert_eq!(m.language.as_deref(), Some("pt-BR"));
    assert_eq!(m.created_at.as_deref(), Some("2024-03-01T10:00:00Z"));
}

#[test]
fn text_in_runs_tabs_and_breaks_is_joined() {
    let body = "<w:p><w:r><w:t>Antes</w:t></w:r><w:r><w:tab/></w:r><w:r><w:t>depois</w:t></w:r><w:r><w:br/><w:t>linha</w:t></w:r></w:p>";
    let parsed = parse_docx(&docx(body)).unwrap();
    assert_eq!(texts(&parsed), ["Antes depois linha"]);
}

#[test]
fn deleted_text_and_fallback_copies_are_not_read() {
    let body = "<w:p><w:r><w:t>Fica</w:t></w:r><w:del><w:r><w:delText>removido</w:delText></w:r></w:del></w:p>\
        <mc:AlternateContent xmlns:mc=\"m\"><mc:Choice><w:p><w:r><w:t>Caixa</w:t></w:r></w:p></mc:Choice>\
        <mc:Fallback><w:p><w:r><w:t>Caixa</w:t></w:r></w:p></mc:Fallback></mc:AlternateContent>";
    let parsed = parse_docx(&docx(body)).unwrap();
    assert_eq!(texts(&parsed), ["Fica", "Caixa"]);
}

#[test]
fn nested_tables_are_flattened_into_their_cell() {
    let inner = table(&[&["interna"]]);
    let body = format!(
        "<w:tbl><w:tr><w:tc>{}{inner}</w:tc><w:tc>{}</w:tc></w:tr><w:tr><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr></w:tbl>",
        p(None, "externa"),
        p(None, "B"),
        p(None, "1"),
        p(None, "2")
    );
    let parsed = parse_docx(&docx(&body)).unwrap();
    assert_eq!(
        texts(&parsed),
        ["Tabela 1: externa interna | B\nexterna interna: 1\nB: 2"]
    );
}

#[test]
fn a_document_without_text_is_empty() {
    assert_eq!(
        parse_docx(&docx("<w:p/><w:p><w:r><w:t>  </w:t></w:r></w:p>")),
        Err(ParseError::Empty)
    );
}

#[test]
fn headings_without_styles_part_fall_back_to_outline_levels() {
    let body = "<w:p><w:pPr><w:outlineLvl w:val=\"0\"/></w:pPr><w:r><w:t>Capítulo</w:t></w:r></w:p>"
        .to_string() + &p(None, "texto");
    let parsed = parse_docx(&docx(&body)).unwrap();
    assert_eq!(
        parsed.blocks().next().unwrap().kind,
        ContentKind::Heading { level: 1 }
    );
    assert_eq!(paths(&parsed), vec![strings(&["Capítulo"])]);
}

#[test]
fn list_items_keep_their_depth() {
    let body = format!("{}{}", li(2, 0, "um"), li(1, 1, "sub"));
    let parsed = parse_docx(&docx(&body)).unwrap();
    let kinds: Vec<_> = parsed.blocks().map(|b| b.kind.clone()).collect();
    assert_eq!(
        kinds[0],
        ContentKind::ListItem {
            ordered: true,
            depth: 0
        }
    );
    assert_eq!(
        kinds[1],
        ContentKind::ListItem {
            ordered: false,
            depth: 1
        }
    );
}

// ---------------------------------------------------------------- XLSX

#[test]
fn an_xlsx_has_one_section_per_visible_sheet() {
    let parsed = parse_xlsx(&vendas()).unwrap();
    assert_eq!(parsed.document_type(), DocumentType::Xlsx);
    assert_eq!(
        paths(&parsed),
        vec![strings(&["Resumo"]), strings(&["Notas"])]
    );
    assert!(!texts(&parsed).iter().any(|t| t.contains("segredo interno")));
    assert!(
        parsed
            .warnings()
            .contains(&ParseWarning::SkippedUnit { index: 2 }),
        "the hidden sheet is reported"
    );
}

#[test]
fn rows_are_records_with_the_header_as_column_names() {
    let parsed = parse_xlsx(&vendas()).unwrap();
    let blocks: Vec<_> = parsed.sections()[0].blocks.iter().collect();
    assert_eq!(blocks.len(), 2, "the header is not a record");
    assert_eq!(
        blocks[0].text,
        "Registro 2:\nRegião: Nordeste\nProduto: Cadeira\nVendas: 1250.5\nRegião (2): 2024-03-01"
    );
    let ContentKind::Record { fields } = &blocks[1].kind else {
        panic!("a record");
    };
    let names: Vec<_> = fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Região", "Produto", "Vendas", "Região (2)"]);
    assert_eq!(
        fields[3].value, "2024-03-02",
        "custom date formats are dates"
    );
}

#[test]
fn rows_are_located_by_sheet_and_excel_row() {
    let parsed = parse_xlsx(&vendas()).unwrap();
    let rows: Vec<_> = parsed.blocks().map(|b| b.location.clone()).collect();
    assert_eq!(
        rows[0],
        SourceLocation::xlsx(1, "Resumo".into(), 2, 2).unwrap()
    );
    assert_eq!(
        rows[1],
        SourceLocation::xlsx(1, "Resumo".into(), 4, 4).unwrap(),
        "empty rows are skipped but numbers are Excel's"
    );
    assert_eq!(
        rows[2],
        SourceLocation::xlsx(3, "Notas".into(), 2, 2).unwrap()
    );
}

#[test]
fn a_header_of_numbers_is_not_a_header() {
    let sheet = Sheet {
        name: "Dados".into(),
        state: "visible",
        rows: vec![
            vec![cell("A1", "n", "10"), cell("B1", "n", "20")],
            vec![cell("A2", "n", "30"), cell("B2", "n", "40")],
        ],
    };
    let parsed = parse_xlsx(&xlsx(&[], &[sheet], &[])).unwrap();
    assert_eq!(parsed.sections()[0].blocks.len(), 2);
    assert!(parsed.warnings().contains(&ParseWarning::NoHeaderRow));
    let ContentKind::Record { fields } = &parsed.sections()[0].blocks[0].kind else {
        panic!("a record");
    };
    assert_eq!(fields[0].name, "Coluna 1");
}

#[test]
fn an_all_text_sheet_has_an_uncertain_header() {
    let sheet = Sheet {
        name: "Texto".into(),
        state: "visible",
        rows: vec![
            vec![cell("A1", "inlineStr", "Nome")],
            vec![cell("A2", "inlineStr", "Ana")],
        ],
    };
    let parsed = parse_xlsx(&xlsx(&[], &[sheet], &[])).unwrap();
    assert!(parsed.warnings().contains(&ParseWarning::UncertainHeader));
}

#[test]
fn booleans_and_errors_are_read() {
    let sheet = Sheet {
        name: "Mix".into(),
        state: "visible",
        rows: vec![
            vec![
                cell("A1", "inlineStr", "Ativo"),
                cell("B1", "inlineStr", "Valor"),
            ],
            vec![cell("A2", "b", "1"), cell("B2", "e", "#DIV/0!")],
        ],
    };
    let parsed = parse_xlsx(&xlsx(&[], &[sheet], &[])).unwrap();
    assert_eq!(
        parsed.sections()[0].blocks[0].text,
        "Registro 2:\nAtivo: Sim"
    );
}

#[test]
fn a_workbook_without_cells_is_empty() {
    let sheet = Sheet {
        name: "Vazia".into(),
        state: "visible",
        rows: vec![],
    };
    assert_eq!(
        parse_xlsx(&xlsx(&[], &[sheet], &[])),
        Err(ParseError::Empty)
    );
}

#[test]
fn styled_numbers_without_a_date_format_stay_numbers() {
    let sheet = Sheet {
        name: "N".into(),
        state: "visible",
        rows: vec![
            vec![cell("A1", "inlineStr", "Valor")],
            vec![styled("A2", "45352", 0)],
        ],
    };
    let parsed = parse_xlsx(&xlsx(&[], &[sheet], &[])).unwrap();
    assert!(
        parsed.sections()[0].blocks[0]
            .text
            .ends_with("Valor: 45352")
    );
}

#[test]
fn a_file_over_the_size_limit_is_refused() {
    let rows = (1..=3)
        .map(|r| vec![cell(&format!("A{r}"), "inlineStr", "x")])
        .collect();
    let sheet = Sheet {
        name: "Grande".into(),
        state: "visible",
        rows,
    };
    let bytes = xlsx(&[], &[sheet], &[]);
    let limits = Limits {
        max_total_bytes: 100,
        ..Limits::default()
    };
    assert_eq!(
        parse_xlsx_with_limits(&bytes, limits),
        Err(ParseError::TooLarge)
    );
}

// ---------------------------------------------------------------- both

#[test]
fn damaged_and_foreign_files_are_invalid() {
    assert_eq!(
        parse_docx(b"not a zip"),
        Err(ParseError::Invalid(DocumentType::Docx))
    );
    assert_eq!(
        parse_xlsx(b"not a zip"),
        Err(ParseError::Invalid(DocumentType::Xlsx))
    );
    // A valid zip that is not the right kind of package.
    let other = zip(&[("hello.txt", b"hi")]);
    assert_eq!(
        parse_docx(&other),
        Err(ParseError::Invalid(DocumentType::Docx))
    );
    assert_eq!(
        parse_xlsx(&other),
        Err(ParseError::Invalid(DocumentType::Xlsx))
    );
}

#[test]
fn protected_files_are_reported_as_encrypted() {
    let mut compound = vec![0xD0, 0xCF, 0x11, 0xE0];
    compound.extend_from_slice(&[0; 64]);
    assert_eq!(parse_docx(&compound), Err(ParseError::Encrypted));
    assert_eq!(parse_xlsx(&compound), Err(ParseError::Encrypted));
}

#[test]
fn zip_bombs_are_refused() {
    let big = format!(
        "<w:document xmlns:w=\"w\"><w:body>{}</w:body></w:document>",
        p(None, &"a".repeat(2_000_000))
    );
    let bytes = zip(&[("word/document.xml", big.as_bytes())]);
    let limits = Limits {
        max_entry_bytes: 1_000_000,
        ..Limits::default()
    };
    assert_eq!(
        parse_docx_with_limits(&bytes, limits),
        Err(ParseError::TooLarge)
    );
    let few = Limits {
        max_entries: 0,
        ..Limits::default()
    };
    assert_eq!(
        parse_docx_with_limits(&contrato(), few),
        Err(ParseError::TooLarge)
    );
}

#[test]
fn errors_do_not_carry_the_content() {
    for error in [
        parse_docx(&docx("")).unwrap_err(),
        parse_xlsx(b"x").unwrap_err(),
    ] {
        assert!(!error.to_string().contains("manutenção"));
    }
}

#[tokio::test]
async fn a_declared_type_that_differs_is_unsupported() {
    let source = DocumentSource::of_type(fixture("contrato.docx"), DocumentType::Xlsx);
    assert_eq!(
        DocxDocumentParser::new().parse(&source).await,
        Err(ParseError::Unsupported)
    );
}

#[test]
fn the_golden_corpus_files_are_the_ones_the_generator_builds() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/golden/corpus");
    for (name, bytes) in common::golden_corpus() {
        assert_eq!(
            std::fs::read(dir.join(name)).unwrap(),
            bytes,
            "{name}: run the generate_fixtures example"
        );
        // And they parse.
        match name.rsplit('.').next() {
            Some("docx") => assert!(parse_docx(&bytes).unwrap().has_text()),
            _ => assert!(parse_xlsx(&bytes).unwrap().has_text()),
        }
    }
}
