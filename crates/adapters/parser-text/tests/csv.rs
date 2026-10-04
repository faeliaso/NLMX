//! CSV semantics on the committed fixtures: header detection, columns, rows, delimiters,
//! encodings, dataset metadata and the incremental stream.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use nlmx_application::ports::{DocumentParser, DocumentSource};
use nlmx_domain::{
    parsed::{ColumnKind, ContentKind, ParseError, ParseWarning, ParsedDocument},
    source::SourceLocation,
};
use nlmx_parser_text::{CsvDocumentParser, CsvLimits, CsvStream, MAX_ROWS};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

async fn parse(name: &str) -> ParsedDocument {
    CsvDocumentParser
        .parse(&DocumentSource::from_path(fixture(name)))
        .await
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn texts(parsed: &ParsedDocument) -> Vec<&str> {
    parsed.blocks().map(|b| b.text.as_str()).collect()
}

fn column_names(parsed: &ParsedDocument) -> Vec<String> {
    parsed.metadata().dataset.as_ref().unwrap().column_names()
}

/// A temporary file removed on drop.
struct TempFile(PathBuf);

impl TempFile {
    fn new(name: &str) -> Self {
        Self(std::env::temp_dir().join(format!("nlmx-csv-{name}-{}.csv", std::process::id())))
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[tokio::test]
async fn a_simple_file_reads_as_registros() {
    let parsed = parse("pessoas.csv").await;
    assert_eq!(
        texts(&parsed),
        [
            "Registro 1:\nNome: João\nIdade: 32\nCidade: Fortaleza\nProfissão: Engenheiro",
            "Registro 2:\nNome: Maria\nIdade: 28\nCidade: Recife\nProfissão: Designer",
        ]
    );
    assert!(parsed.warnings().is_empty());
    let dataset = parsed.metadata().dataset.as_ref().unwrap();
    assert!(dataset.has_header);
    assert_eq!(dataset.delimiter, ',');
    assert_eq!(dataset.row_count, Some(2));
    assert_eq!(
        column_names(&parsed),
        ["Nome", "Idade", "Cidade", "Profissão"]
    );
    let kinds: Vec<_> = dataset.columns.iter().map(|c| c.kind).collect();
    assert_eq!(
        kinds,
        [
            ColumnKind::Text,
            ColumnKind::Number,
            ColumnKind::Text,
            ColumnKind::Text
        ]
    );
    let rows: Vec<_> = parsed.blocks().map(|b| b.location.clone()).collect();
    assert_eq!(
        rows,
        [
            SourceLocation::csv(1, 1).unwrap(),
            SourceLocation::csv(2, 2).unwrap()
        ]
    );
}

#[tokio::test]
async fn empty_fields_stay_in_the_structure_and_leave_the_text() {
    let parsed = parse("vazios.csv").await;
    // The blank line is not a record; the row of empty cells counts but has no block.
    assert_eq!(
        texts(&parsed),
        [
            "Registro 1:\nnome: Ana\ncidade: Natal",
            "Registro 2:\nidade: 41",
            "Registro 3:\nnome: Bia\nidade: 29",
            "Registro 5:\nnome: Carlos\ncidade: Salvador",
        ]
    );
    let ContentKind::Record { fields } = &parsed.blocks().next().unwrap().kind else {
        panic!("a record")
    };
    assert_eq!(fields.len(), 3);
    assert_eq!(fields[1].value, "");
    let rows: Vec<_> = parsed.blocks().map(|b| b.location.clone()).collect();
    assert_eq!(rows[3], SourceLocation::csv(5, 5).unwrap());
    assert_eq!(
        parsed.metadata().dataset.as_ref().unwrap().row_count,
        Some(5)
    );
}

#[tokio::test]
async fn many_columns_are_all_kept() {
    let parsed = parse("muitas-colunas.csv").await;
    let names = column_names(&parsed);
    assert_eq!(names.len(), 80);
    assert_eq!(
        (names[0].as_str(), names[79].as_str()),
        ("campo_01", "campo_80")
    );
    assert_eq!(parsed.blocks().count(), 3);
    let first = parsed.blocks().next().unwrap();
    assert_eq!(
        first.text.lines().count(),
        81,
        "the title line and one per column"
    );
    assert!(first.text.contains("\ncampo_80: 180"));
    let ContentKind::Record { fields } = &first.kind else {
        panic!("a record")
    };
    assert_eq!(fields.len(), 80);
}

#[tokio::test]
async fn accents_and_legacy_encodings_are_read() {
    let parsed = parse("vendas.csv").await;
    assert_eq!(
        texts(&parsed)[0],
        "Registro 1:\nproduto: Café\nquantidade: 3\npreço: 12,50\nobservação: Torra média embalagem de 500 g"
    );
    assert_eq!(parsed.metadata().dataset.as_ref().unwrap().delimiter, ';');

    let legacy = TempFile::new("latin1");
    fs::write(&legacy.0, b"produto;pre\xE7o\nCaf\xE9;12,50\nCh\xE1;8,00\n").unwrap();
    let parsed = CsvDocumentParser
        .parse(&DocumentSource::from_path(&legacy.0))
        .await
        .unwrap();
    assert_eq!(parsed.warnings(), [ParseWarning::FallbackEncoding]);
    assert_eq!(
        texts(&parsed)[0],
        "Registro 1:\nproduto: Café\npreço: 12,50"
    );
}

#[tokio::test]
async fn alternative_delimiters_are_detected() {
    for (name, delimiter, first) in [
        ("vendas.csv", ';', "Registro 1:\nproduto: Café"),
        (
            "pipe.csv",
            '|',
            "Registro 1:\nproduto: Caneta\nquantidade: 10\npreço: 2,50",
        ),
        (
            "tabulado.csv",
            '\t',
            "Registro 1:\ncidade: Fortaleza\npopulação: 2686612\nuf: CE",
        ),
    ] {
        let parsed = parse(name).await;
        let dataset = parsed.metadata().dataset.as_ref().unwrap();
        assert_eq!(dataset.delimiter, delimiter, "{name}");
        assert!(dataset.has_header, "{name}");
        assert!(texts(&parsed)[0].starts_with(first), "{name}");
    }
}

#[tokio::test]
async fn a_headerless_file_is_recognized_by_its_data() {
    let parsed = parse("dados-sem-cabecalho.csv").await;
    assert_eq!(parsed.warnings(), [ParseWarning::NoHeaderRow]);
    let dataset = parsed.metadata().dataset.as_ref().unwrap();
    assert!(!dataset.has_header);
    assert_eq!(column_names(&parsed), ["coluna 1", "coluna 2", "coluna 3"]);
    let kinds: Vec<_> = dataset.columns.iter().map(|c| c.kind).collect();
    assert_eq!(
        kinds,
        [ColumnKind::Text, ColumnKind::Number, ColumnKind::Date]
    );
    // The first line is data row 1, not a header.
    assert_eq!(
        texts(&parsed)[0],
        "Registro 1:\ncoluna 1: Ana\ncoluna 2: 31\ncoluna 3: 2020-03-15"
    );
    assert_eq!(parsed.blocks().count(), 3);
}

#[tokio::test]
async fn the_header_is_confident_uncertain_or_absent() {
    // Names above figures: confirmed by the data, no warning.
    assert!(parse("pessoas.csv").await.warnings().is_empty());
    // Text above text: used, but flagged.
    let uncertain = parse("somente-texto.csv").await;
    assert_eq!(uncertain.warnings(), [ParseWarning::UncertainHeader]);
    assert_eq!(column_names(&uncertain), ["nome", "cidade", "profissão"]);
    assert_eq!(
        texts(&uncertain)[0],
        "Registro 1:\nnome: João\ncidade: Fortaleza\nprofissão: Engenheiro"
    );
    // Data in the first row: no header.
    assert_eq!(
        parse("sem-cabecalho.csv").await.warnings(),
        [ParseWarning::NoHeaderRow]
    );
}

#[tokio::test]
async fn parse_equals_parse_bytes() {
    for name in [
        "pessoas.csv",
        "vazios.csv",
        "vendas.csv",
        "pipe.csv",
        "tabulado.csv",
        "dados-sem-cabecalho.csv",
        "somente-texto.csv",
        "muitas-colunas.csv",
    ] {
        let from_bytes = CsvDocumentParser::parse_bytes(&fs::read(fixture(name)).unwrap()).unwrap();
        assert_eq!(parse(name).await, from_bytes, "{name}");
    }
}

#[test]
fn the_stream_gives_the_dataset_before_any_row_and_the_count_at_the_end() {
    let mut stream = CsvStream::open(&fixture("pessoas.csv")).unwrap();
    assert_eq!(stream.dataset().columns.len(), 4);
    assert_eq!(stream.row_count(), None);
    let first = stream.next().unwrap().unwrap();
    assert!(first.text.starts_with("Registro 1:"));
    assert_eq!(stream.row_count(), None);
    assert_eq!(stream.by_ref().count(), 1);
    assert_eq!(stream.row_count(), Some(2));
}

#[test]
fn later_bytes_do_not_change_the_first_row() {
    // Opening validates the encoding of the whole file in constant memory; a late byte that is
    // not UTF-8 makes it Windows-1252, and the first row is still there, whole.
    let file = TempFile::new("late");
    let mut bytes = b"id,nome\n1,Ana\n".to_vec();
    for i in 2..2_000 {
        bytes.extend(format!("{i},pessoa {i}\n").bytes());
    }
    bytes.extend(b"2000,Jos\xE9\n");
    fs::write(&file.0, &bytes).unwrap();
    let mut stream = CsvStream::open(&file.0).unwrap();
    assert_eq!(
        stream.next().unwrap().unwrap().text,
        "Registro 1:\nid: 1\nnome: Ana"
    );
    assert_eq!(stream.warnings(), [ParseWarning::FallbackEncoding]);
    assert_eq!(stream.count(), 1_999);
}

#[test]
fn a_mid_file_error_reaches_the_reader_of_the_stream() {
    let file = TempFile::new("limit");
    fs::write(&file.0, "a,b\n1,x\n2,y\n3,z\n4,w\n").unwrap();
    let limits = CsvLimits {
        max_rows: 2,
        ..CsvLimits::default()
    };
    let mut stream = CsvStream::open_with_limits(&file.0, limits).unwrap();
    assert!(stream.next().unwrap().is_ok());
    assert!(stream.next().unwrap().is_ok());
    assert_eq!(stream.next(), Some(Err(ParseError::TooLarge)));
    assert!(stream.next().is_none());
}

#[tokio::test]
async fn a_file_larger_than_the_document_limit_is_still_streamed() {
    let file = TempFile::new("large");
    let rows = MAX_ROWS + 50_000;
    {
        let mut out = std::io::BufWriter::new(fs::File::create(&file.0).unwrap());
        writeln!(out, "id,nome,valor").unwrap();
        for i in 1..=rows {
            writeln!(out, "{i},pessoa {i},{}", i % 97).unwrap();
        }
    }

    // As a document it is too big to hold...
    let source = DocumentSource::from_path(&file.0);
    assert_eq!(
        CsvDocumentParser.parse(&source).await,
        Err(ParseError::TooLarge)
    );

    // ...but the stream reads every row, one at a time, numbered without gaps.
    let mut stream = CsvStream::open(&file.0).unwrap();
    let mut expected = 0u32;
    for block in stream.by_ref() {
        let block = block.unwrap();
        expected += 1;
        assert_eq!(
            block.location,
            SourceLocation::csv(expected, expected).unwrap()
        );
    }
    assert_eq!(expected, rows);
    assert_eq!(stream.row_count(), Some(rows));
}
