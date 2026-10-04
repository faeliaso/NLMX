//! A CSV file read row by row: detection from the beginning of the file, then one record block
//! at a time. Memory is that of one record, not of the file.

use std::{
    fs::File,
    io::{self, Cursor, Read},
    path::Path,
    sync::Arc,
};

use csv::{ReaderBuilder, StringRecord, StringRecordsIntoIter};
use encoding_rs_io::DecodeReaderBytesBuilder;
use nlmx_domain::{
    document_type::DocumentType,
    parsed::{
        ContentBlock, ContentKind, DatasetColumn, DatasetMetadata, ParseError, ParseWarning,
        RecordField,
    },
    source::SourceLocation,
};

use crate::sniff::{
    Header, SAMPLE_BYTES, column_kinds, decide_header, detect_charset, detect_delimiter,
    sample_records,
};

const KIND: DocumentType = DocumentType::Csv;

/// Most bytes read from one file by default (1 GiB).
pub const MAX_STREAM_BYTES: u64 = 1024 * 1024 * 1024;
/// Most data rows read from one file by default.
pub const MAX_STREAM_ROWS: u32 = 5_000_000;

/// What a stream refuses to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CsvLimits {
    pub max_bytes: u64,
    /// Data rows (blank-cell rows included, header excluded); one more ⇒ `TooLarge`.
    pub max_rows: u32,
}

impl Default for CsvLimits {
    fn default() -> Self {
        Self {
            max_bytes: MAX_STREAM_BYTES,
            max_rows: MAX_STREAM_ROWS,
        }
    }
}

type Source = Box<dyn Read + Send>;

/// The records of a CSV file as `Record` blocks, in order.
///
/// Opening reads the whole file once to pick its encoding (constant memory) and then the start
/// of it to detect the delimiter, the header and the column kinds; iterating reads the rest
/// lazily. Each item is the block of a data row (`Registro n:` and one `Coluna: valor` line per
/// non-empty cell), located by [`SourceLocation::csv`]; rows whose cells are all empty are
/// skipped but counted, and blank lines are not records. After an `Err` the stream ends.
pub struct CsvStream {
    records: StringRecordsIntoIter<Source>,
    dataset: DatasetMetadata,
    warnings: Vec<ParseWarning>,
    rows: u32,
    max_rows: u32,
    done: bool,
}

fn io_error(error: io::Error) -> ParseError {
    match error.kind() {
        io::ErrorKind::NotFound => ParseError::NotFound,
        _ => ParseError::Engine("não foi possível ler o arquivo".into()),
    }
}

fn csv_error(error: csv::Error) -> ParseError {
    if error.is_io_error() {
        ParseError::Engine("não foi possível ler o arquivo".into())
    } else {
        ParseError::Invalid(KIND)
    }
}

impl CsvStream {
    pub fn open(path: &Path) -> Result<Self, ParseError> {
        Self::open_with_limits(path, CsvLimits::default())
    }

    pub fn open_with_limits(path: &Path, limits: CsvLimits) -> Result<Self, ParseError> {
        let metadata = std::fs::metadata(path).map_err(io_error)?;
        if !metadata.is_file() {
            return Err(ParseError::NotFound);
        }
        if metadata.len() > limits.max_bytes {
            return Err(ParseError::TooLarge);
        }
        let path = path.to_path_buf();
        Self::build(
            move || File::open(&path).map(|file| Box::new(file) as Source),
            limits.max_rows,
        )
    }

    /// A stream over bytes already in memory (at most 64 MiB, the limit of the other text
    /// parsers).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ParseError> {
        Self::from_bytes_with_limits(
            bytes,
            CsvLimits {
                max_bytes: crate::MAX_BYTES,
                ..CsvLimits::default()
            },
        )
    }

    pub fn from_bytes_with_limits(bytes: &[u8], limits: CsvLimits) -> Result<Self, ParseError> {
        if bytes.len() as u64 > limits.max_bytes {
            return Err(ParseError::TooLarge);
        }
        let data: Arc<[u8]> = Arc::from(bytes);
        Self::build(
            move || Ok(Box::new(Cursor::new(data.clone())) as Source),
            limits.max_rows,
        )
    }

    fn build(open: impl Fn() -> io::Result<Source>, max_rows: u32) -> Result<Self, ParseError> {
        let charset = detect_charset(open().map_err(io_error)?)?;
        let mut decoded = DecodeReaderBytesBuilder::new()
            .encoding(Some(charset.encoding))
            .bom_override(true)
            .build(open().map_err(io_error)?);

        // The start of the (decoded) file decides delimiter, header and column kinds.
        let mut sample = Vec::new();
        (&mut decoded)
            .take(SAMPLE_BYTES)
            .read_to_end(&mut sample)
            .map_err(io_error)?;
        let truncated = sample.len() as u64 == SAMPLE_BYTES;
        let mut text = String::from_utf8_lossy(&sample).into_owned();
        if truncated && let Some(end) = text.rfind('\n') {
            text.truncate(end + 1);
        }
        if !truncated && text.trim().is_empty() {
            return Err(ParseError::Empty);
        }
        let delimiter = detect_delimiter(&text);
        let records = sample_records(&text, delimiter);
        let header = decide_header(&records);
        let width = records.iter().map(Vec::len).max().unwrap_or(1).max(1);
        let (names, data): (Vec<String>, &[Vec<String>]) = match header {
            Header::Absent => (Vec::new(), &records),
            _ => (records[0].clone(), &records[1..]),
        };
        let kinds = column_kinds(data, width);
        let columns = (0..width)
            .map(|column| DatasetColumn {
                name: column_name(&names, column),
                kind: kinds[column],
            })
            .collect();

        let mut warnings = Vec::new();
        if charset.fallback {
            warnings.push(ParseWarning::FallbackEncoding);
        }
        match header {
            Header::Confident => {}
            Header::Uncertain => warnings.push(ParseWarning::UncertainHeader),
            Header::Absent => warnings.push(ParseWarning::NoHeaderRow),
        }

        let source: Source = Box::new(Cursor::new(sample).chain(decoded));
        let mut records = ReaderBuilder::new()
            .delimiter(delimiter)
            .has_headers(false)
            .flexible(true)
            .from_reader(source)
            .into_records();
        if header != Header::Absent
            && let Some(Err(error)) = records.next()
        {
            return Err(csv_error(error));
        }
        Ok(Self {
            records,
            dataset: DatasetMetadata {
                columns,
                delimiter: char::from(delimiter),
                has_header: header != Header::Absent,
                row_count: None,
            },
            warnings,
            rows: 0,
            max_rows,
            done: false,
        })
    }

    /// Facts about the file known from its start; `row_count` is set once the stream has been
    /// read to the end.
    pub fn dataset(&self) -> &DatasetMetadata {
        &self.dataset
    }

    pub fn warnings(&self) -> &[ParseWarning] {
        &self.warnings
    }

    /// Data rows in the file (blank-cell rows included): `None` until the stream is exhausted.
    pub fn row_count(&self) -> Option<u32> {
        self.dataset.row_count
    }

    fn block(&self, row: u32, record: &StringRecord) -> Result<Option<ContentBlock>, ParseError> {
        let mut fields = Vec::with_capacity(record.len());
        for (column, cell) in record.iter().enumerate() {
            if cell.contains('\0') {
                return Err(ParseError::Invalid(KIND));
            }
            let name = self
                .dataset
                .columns
                .get(column)
                .map_or_else(|| positional(column), |c| c.name.clone());
            fields.push(RecordField {
                name,
                value: cell.trim().to_string(),
            });
        }
        let mut text = String::new();
        for field in fields.iter().filter(|f| !f.value.is_empty()) {
            text.push('\n');
            text.push_str(&field.name);
            text.push_str(": ");
            text.push_str(&field.value.split_whitespace().collect::<Vec<_>>().join(" "));
        }
        if text.is_empty() {
            return Ok(None);
        }
        Ok(Some(ContentBlock {
            kind: ContentKind::Record { fields },
            text: format!("Registro {row}:{text}"),
            location: SourceLocation::csv(row, row).map_err(|_| ParseError::Invalid(KIND))?,
        }))
    }
}

fn positional(column: usize) -> String {
    format!("coluna {}", column + 1)
}

fn column_name(names: &[String], column: usize) -> String {
    names
        .get(column)
        .filter(|name| !name.is_empty())
        .cloned()
        .unwrap_or_else(|| positional(column))
}

impl Iterator for CsvStream {
    type Item = Result<ContentBlock, ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        loop {
            let record = match self.records.next() {
                None => {
                    self.done = true;
                    self.dataset.row_count = Some(self.rows);
                    return None;
                }
                Some(Err(error)) => {
                    self.done = true;
                    return Some(Err(csv_error(error)));
                }
                Some(Ok(record)) => record,
            };
            if self.rows >= self.max_rows {
                self.done = true;
                return Some(Err(ParseError::TooLarge));
            }
            self.rows += 1;
            match self.block(self.rows, &record) {
                Ok(Some(block)) => return Some(Ok(block)),
                Ok(None) => {}
                Err(error) => {
                    self.done = true;
                    return Some(Err(error));
                }
            }
        }
    }
}

impl std::iter::FusedIterator for CsvStream {}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(text: &str) -> CsvStream {
        CsvStream::from_bytes(text.as_bytes()).unwrap()
    }

    fn texts(stream: CsvStream) -> Vec<String> {
        stream.map(|b| b.unwrap().text).collect()
    }

    #[test]
    fn a_row_reads_as_a_registro_with_one_line_per_column() {
        let s = stream(
            "Nome,Idade,Cidade,Profissão\nJoão,32,Fortaleza,Engenheiro\nMaria,28,Recife,Designer\n",
        );
        assert!(s.dataset().has_header);
        assert_eq!(
            texts(s),
            [
                "Registro 1:\nNome: João\nIdade: 32\nCidade: Fortaleza\nProfissão: Engenheiro",
                "Registro 2:\nNome: Maria\nIdade: 28\nCidade: Recife\nProfissão: Designer",
            ]
        );
    }

    #[test]
    fn blocks_keep_the_structure_and_the_row_location() {
        let blocks: Vec<_> = stream("a,b\n1,x\n2,\n3,z\n").map(Result::unwrap).collect();
        let rows: Vec<_> = blocks.iter().map(|b| b.location.clone()).collect();
        assert_eq!(
            rows,
            [
                SourceLocation::csv(1, 1).unwrap(),
                SourceLocation::csv(2, 2).unwrap(),
                SourceLocation::csv(3, 3).unwrap(),
            ]
        );
        // An empty cell stays in the structure, not in the text.
        let ContentKind::Record { fields } = &blocks[1].kind else {
            panic!("{:?}", blocks[1].kind)
        };
        assert_eq!(
            fields,
            &[
                RecordField {
                    name: "a".into(),
                    value: "2".into()
                },
                RecordField {
                    name: "b".into(),
                    value: String::new()
                },
            ]
        );
        assert_eq!(blocks[1].text, "Registro 2:\na: 2");
    }

    #[test]
    fn values_are_trimmed_and_their_whitespace_collapsed_in_the_text() {
        let blocks: Vec<_> = stream("id,nota\n1,\"  linha um\n  linha   dois \"\n2,ok\n")
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            blocks[0].text,
            "Registro 1:\nid: 1\nnota: linha um linha dois"
        );
        let ContentKind::Record { fields } = &blocks[0].kind else {
            panic!()
        };
        assert_eq!(fields[1].value, "linha um\n  linha   dois");
    }

    #[test]
    fn the_row_count_is_known_only_after_the_end() {
        let mut s = stream("a,b\n1,2\n,\n3,4\n");
        assert_eq!(s.row_count(), None);
        assert!(s.next().is_some());
        assert_eq!(s.row_count(), None);
        assert_eq!(s.by_ref().count(), 1, "the all-empty row is skipped");
        // Three data rows, the middle one skipped but counted.
        assert_eq!(s.row_count(), Some(3));
        assert_eq!(s.dataset().row_count, Some(3));
        assert!(s.next().is_none(), "fused");
    }

    #[test]
    fn the_dataset_describes_the_file() {
        let s = stream(
            "produto;quantidade;preço;ativo;desde\nA;3;1,5;sim;2024-01-02\nB;4;2,5;não;2024-02-03\n",
        );
        let dataset = s.dataset();
        assert_eq!(dataset.delimiter, ';');
        assert!(dataset.has_header);
        let columns: Vec<_> = dataset
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.kind))
            .collect();
        use nlmx_domain::parsed::ColumnKind::*;
        assert_eq!(
            columns,
            [
                ("produto", Text),
                ("quantidade", Number),
                ("preço", Number),
                ("ativo", Boolean),
                ("desde", Date),
            ]
        );
        assert!(s.warnings().is_empty());
    }

    #[test]
    fn a_file_without_header_gets_positional_names_and_a_warning() {
        let s = stream("Ana,31,2020-03-15\nBruno,45,2019-11-02\n");
        assert!(!s.dataset().has_header);
        assert_eq!(s.warnings(), [ParseWarning::NoHeaderRow]);
        let names: Vec<_> = s.dataset().columns.iter().map(|c| c.name.clone()).collect();
        assert_eq!(names, ["coluna 1", "coluna 2", "coluna 3"]);
        assert_eq!(
            texts(s)[0],
            "Registro 1:\ncoluna 1: Ana\ncoluna 2: 31\ncoluna 3: 2020-03-15"
        );
    }

    #[test]
    fn an_all_text_header_is_used_but_flagged() {
        let s = stream("nome,cidade\nJoão,Fortaleza\nMaria,Recife\n");
        assert!(s.dataset().has_header);
        assert_eq!(s.warnings(), [ParseWarning::UncertainHeader]);
    }

    #[test]
    fn ragged_rows_get_positional_names_beyond_the_header() {
        let s = stream("a,b,c\n1,2\n3,4,5,6\n");
        assert_eq!(s.dataset().columns.len(), 4);
        assert_eq!(
            texts(s),
            [
                "Registro 1:\na: 1\nb: 2",
                "Registro 2:\na: 3\nb: 4\nc: 5\ncoluna 4: 6"
            ]
        );
    }

    #[test]
    fn encodings_are_read_incrementally() {
        let units: Vec<u16> = "nome;valor\nAção;1\n".encode_utf16().collect();
        let mut le = vec![0xFF, 0xFE];
        le.extend(units.iter().flat_map(|u| u.to_le_bytes()));
        assert_eq!(
            texts(CsvStream::from_bytes(&le).unwrap()),
            ["Registro 1:\nnome: Ação\nvalor: 1"]
        );

        let s = CsvStream::from_bytes(b"nome;valor\nA\xE7\xE3o;1\n").unwrap();
        assert_eq!(s.warnings(), [ParseWarning::FallbackEncoding]);
        assert_eq!(texts(s), ["Registro 1:\nnome: Ação\nvalor: 1"]);

        // A UTF-8 BOM is not part of the first column name.
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend("nome,valor\nAna,1\n".as_bytes());
        let s = CsvStream::from_bytes(&bom).unwrap();
        assert_eq!(s.dataset().columns[0].name, "nome");
    }

    #[test]
    fn empty_and_binary_input_are_refused() {
        let open = |bytes: &[u8]| CsvStream::from_bytes(bytes).err();
        assert_eq!(open(b""), Some(ParseError::Empty));
        assert_eq!(open(b" \n\n"), Some(ParseError::Empty));
        assert_eq!(open(b"a,b\n\0,1\n"), Some(ParseError::Invalid(KIND)));
        // Only empty cells: a stream with nothing to read.
        assert_eq!(texts(stream(",,\n,,\n")), Vec::<String>::new());
    }

    #[test]
    fn the_row_limit_stops_the_stream_with_an_error() {
        let limits = CsvLimits {
            max_rows: 3,
            ..CsvLimits::default()
        };
        let mut s =
            CsvStream::from_bytes_with_limits(b"a,b\n1,x\n2,y\n3,z\n4,w\n5,v\n", limits).unwrap();
        for expected in 1..=3 {
            let block = s.next().unwrap().unwrap();
            assert_eq!(
                block.location,
                SourceLocation::csv(expected, expected).unwrap()
            );
        }
        assert_eq!(s.next(), Some(Err(ParseError::TooLarge)));
        assert!(s.next().is_none(), "the stream ends after an error");
        assert_eq!(s.row_count(), None);
    }

    #[test]
    fn a_nul_in_a_utf16_field_fails_the_stream_midway() {
        let units: Vec<u16> = "a,b\n1,x\n2,y\u{0}z\n3,w\n".encode_utf16().collect();
        let mut le = vec![0xFF, 0xFE];
        le.extend(units.iter().flat_map(|u| u.to_le_bytes()));
        let mut s = CsvStream::from_bytes(&le).unwrap();
        assert!(s.next().unwrap().is_ok());
        assert_eq!(s.next(), Some(Err(ParseError::Invalid(KIND))));
        assert!(s.next().is_none());
    }

    #[test]
    fn size_limits_are_checked_before_reading() {
        let limits = CsvLimits {
            max_bytes: 4,
            ..CsvLimits::default()
        };
        assert_eq!(
            CsvStream::from_bytes_with_limits(b"a,b\n1,2\n", limits).err(),
            Some(ParseError::TooLarge)
        );
        assert_eq!(
            CsvStream::open(Path::new("/definitely/not/here.csv")).err(),
            Some(ParseError::NotFound)
        );
    }
}
