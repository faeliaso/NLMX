//! `XlsxDocumentParser` on one small fixture per scenario: one sheet, several sheets, header,
//! empty cells, numbers, dates, strings, an Excel table and many rows.

mod common;

use std::path::{Path, PathBuf};

use common::{xlsx, xlsx_fixtures};
use nlmx_application::ports::{DocumentParser, DocumentSource};
use nlmx_domain::{
    document_type::DocumentType,
    parsed::{ContentKind, ParseWarning, ParsedDocument},
    source::SourceLocation,
};
use nlmx_parser_office::{XlsxDocumentParser, parse_xlsx};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn parse_fixture(name: &str) -> ParsedDocument {
    parse_xlsx(&std::fs::read(fixture(name)).unwrap()).unwrap()
}

fn texts(parsed: &ParsedDocument) -> Vec<&str> {
    parsed.blocks().map(|b| b.text.as_str()).collect()
}

fn sheet_names(parsed: &ParsedDocument) -> Vec<String> {
    parsed
        .sections()
        .iter()
        .map(|s| s.path[0].clone())
        .collect()
}

fn values_of(parsed: &ParsedDocument, column: &str) -> Vec<String> {
    parsed
        .blocks()
        .filter_map(|b| match &b.kind {
            ContentKind::Record { fields } => fields
                .iter()
                .find(|f| f.name == column)
                .map(|f| f.value.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn every_xlsx_fixture_is_the_one_the_generator_builds() {
    for (name, bytes) in xlsx_fixtures() {
        assert_eq!(
            std::fs::read(fixture(name)).unwrap(),
            bytes,
            "{name}: run the generate_fixtures example"
        );
    }
}

#[tokio::test]
async fn every_xlsx_fixture_goes_through_the_parser_port() {
    for (name, _) in xlsx_fixtures() {
        let parsed = XlsxDocumentParser::new()
            .parse(&DocumentSource::from_path(fixture(name)))
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(parsed.document_type(), DocumentType::Xlsx, "{name}");
        assert_eq!(parsed.validate(), Ok(()), "{name}");
        assert!(parsed.has_text(), "{name}");
        assert!(!parsed.document_type().previewable());
    }
}

#[test]
fn one_sheet_keeps_workbook_sheet_row_column_and_value() {
    let parsed = parse_fixture("uma-aba.xlsx");
    assert_eq!(sheet_names(&parsed), ["Janeiro"]);
    let block = parsed.blocks().next().unwrap();
    assert_eq!(
        block.text,
        "Registro 2:\nProduto: Notebook\nQuantidade: 10\nValor: 5000"
    );
    let ContentKind::Record { fields } = &block.kind else {
        panic!("a record, not flat text");
    };
    let pairs: Vec<_> = fields
        .iter()
        .map(|f| (f.name.as_str(), f.value.as_str()))
        .collect();
    assert_eq!(
        pairs,
        [
            ("Produto", "Notebook"),
            ("Quantidade", "10"),
            ("Valor", "5000")
        ]
    );
    assert_eq!(
        block.location,
        SourceLocation::xlsx(1, "Janeiro".into(), 2, 2).unwrap()
    );
    assert_eq!(block.location.label(), "Janeiro, linha 2");
}

#[test]
fn every_sheet_stays_identifiable() {
    let parsed = parse_fixture("varias-abas.xlsx");
    assert_eq!(sheet_names(&parsed), ["Janeiro", "Fevereiro", "Março"]);
    let located: Vec<_> = parsed
        .blocks()
        .map(|b| (b.location.label(), b.text.clone()))
        .collect();
    assert_eq!(
        located[0],
        (
            "Janeiro, linha 2".into(),
            "Registro 2:\nProduto: Notebook\nQuantidade: 10".into()
        )
    );
    assert_eq!(located[1].0, "Fevereiro, linha 2");
    assert_eq!(located[2].0, "Março, linha 2");
    for (i, block) in parsed.blocks().enumerate() {
        let SourceLocation::Xlsx { sheet_index, .. } = &block.location else {
            panic!("an XLSX location");
        };
        assert_eq!(*sheet_index as usize, i + 1);
    }
}

#[test]
fn the_header_names_the_columns_instead_of_a_bare_row() {
    let parsed = parse_fixture("cabecalho.xlsx");
    assert_eq!(
        texts(&parsed),
        [
            "Registro 2:\nNome: João\nCidade: Fortaleza\nCargo: Engenheiro",
            "Registro 3:\nNome: Maria\nCidade: Recife\nCargo: Analista",
        ]
    );
    assert!(!texts(&parsed).iter().any(|t| t.contains(" | ")));
    assert!(parsed.warnings().contains(&ParseWarning::UncertainHeader));
}

#[test]
fn empty_cells_make_no_noise_and_keep_the_columns_in_place() {
    let parsed = parse_fixture("vazias.xlsx");
    assert_eq!(
        texts(&parsed),
        [
            "Registro 2:\nNome: Ana\nCargo: Gerente",
            "Registro 3:\nNome: Bia\nCidade: Recife",
            "Registro 5:\nNome: Caio\nCidade: Natal\nColuna 3: sem nome de coluna\nCargo: Dev",
        ],
        "empty cells are left out, the blank row 4 is skipped, rows keep Excel's numbers"
    );
    let ContentKind::Record { fields } = &parsed.blocks().next().unwrap().kind else {
        panic!("a record");
    };
    let names: Vec<_> = fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Nome", "Cidade", "Coluna 3", "Cargo"]);
}

#[test]
fn numbers_are_kept_as_the_file_stores_them() {
    let parsed = parse_fixture("numeros.xlsx");
    assert_eq!(
        values_of(&parsed, "Valor"),
        [
            "10",
            "1250.5",
            "-3",
            "1.5E-5",
            "12345678901",
            "0.1",
            "6000", // a formula: its cached result
            "Sim",
        ]
    );
}

#[test]
fn dates_are_written_in_a_readable_way() {
    let parsed = parse_fixture("datas.xlsx");
    assert_eq!(
        values_of(&parsed, "Quando"),
        [
            "2024-03-01",
            "2024-03-01 12:00",
            "2024-04-01", // a custom dd/mm/yyyy format
            "45352",      // the same serial without a date format stays a number
        ]
    );
}

#[test]
fn strings_keep_their_accents_entities_and_collapse_whitespace() {
    let parsed = parse_fixture("textos.xlsx");
    assert_eq!(
        texts(&parsed),
        [
            "Registro 2:\nDescrição: Ação & reação\nObservação: espaços extras",
            "Registro 3:\nDescrição: Cadeira de escritório\nObservação: linha um linha dois",
        ]
    );
}

#[test]
fn an_excel_table_gives_the_header_and_keeps_the_rows_around_it() {
    let parsed = parse_fixture("tabela-excel.xlsx");
    assert_eq!(
        texts(&parsed),
        [
            "Linha 1: Relatório de vendas 2024",
            "Registro 4:\nProduto: Notebook\nRegião: Norte\nTotal: 100",
            "Registro 5:\nProduto: Monitor\nRegião: Sul\nTotal: 200",
            "Registro 6:\nProduto: Teclado\nRegião: Leste\nTotal: 300",
            "Linha 8: Valores em reais",
        ]
    );
    let kinds: Vec<bool> = parsed
        .blocks()
        .map(|b| matches!(b.kind, ContentKind::Record { .. }))
        .collect();
    assert_eq!(kinds, [false, true, true, true, false]);
    assert!(
        !parsed.warnings().contains(&ParseWarning::NoHeaderRow),
        "the table declares its header"
    );
    // The title keeps its own row number.
    assert_eq!(
        parsed.blocks().next().unwrap().location,
        SourceLocation::xlsx(1, "Relatório".into(), 1, 1).unwrap()
    );
}

#[test]
fn many_rows_keep_their_numbers_and_order() {
    let parsed = parse_fixture("muitas-linhas.xlsx");
    let rows: Vec<u32> = parsed
        .blocks()
        .map(|b| match &b.location {
            SourceLocation::Xlsx { row_start, .. } => *row_start,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(rows.len(), 300);
    assert_eq!(rows.first(), Some(&2));
    assert_eq!(rows.last(), Some(&301));
    assert!(rows.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(values_of(&parsed, "Item")[299], "Item número 300");
}

#[test]
fn repeated_column_names_do_not_collide() {
    use common::{Sheet, cell};
    let sheet = Sheet {
        name: "Dup".into(),
        state: "visible",
        rows: vec![
            vec![
                cell("A1", "inlineStr", "Valor"),
                cell("B1", "inlineStr", "Valor"),
            ],
            vec![cell("A2", "n", "1"), cell("B2", "n", "2")],
        ],
    };
    let parsed = parse_xlsx(&xlsx(&[], &[sheet], &[])).unwrap();
    assert_eq!(texts(&parsed), ["Registro 2:\nValor: 1\nValor (2): 2"]);
}

#[test]
fn a_table_whose_header_row_is_missing_falls_back_to_the_first_row() {
    // Table says A9:B9, but there is nothing on row 9: the sheet is read as if it had no table.
    let parsed = {
        use common::{Sheet, cell};
        let sheet = Sheet {
            name: "S".into(),
            state: "visible",
            rows: vec![
                vec![
                    cell("A1", "inlineStr", "Nome"),
                    cell("B1", "inlineStr", "Idade"),
                ],
                vec![cell("A2", "inlineStr", "Ana"), cell("B2", "n", "30")],
            ],
        };
        let rels = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/table\" Target=\"../tables/table1.xml\"/></Relationships>";
        let table = "<table xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" ref=\"A9:B9\"/>";
        parse_xlsx(&xlsx(
            &[],
            &[sheet],
            &[
                ("xl/worksheets/_rels/sheet1.xml.rels", rels.as_bytes()),
                ("xl/tables/table1.xml", table.as_bytes()),
            ],
        ))
        .unwrap()
    };
    assert_eq!(texts(&parsed), ["Registro 2:\nNome: Ana\nIdade: 30"]);
}
