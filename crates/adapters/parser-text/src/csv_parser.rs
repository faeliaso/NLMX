//! CSV: one `Record` block per data row (`Registro n:` and a line per column), with the dataset's
//! columns, delimiter and header kept in the metadata. The reading is [`CsvStream`]'s; this
//! parser collects it into a `ParsedDocument`.

use std::path::Path;

use nlmx_application::ports::{BoxFuture, DocumentParser, DocumentSource};
use nlmx_domain::{
    document_type::DocumentType,
    parsed::{DocumentMetadata, ParseError, ParsedDocument},
};

use crate::{
    flat_document,
    stream::{CsvLimits, CsvStream},
};

const VERSION: u32 = 2;
const KIND: DocumentType = DocumentType::Csv;

/// Most data rows collected into one `ParsedDocument`; the stream reads more (to chunk a big
/// file without holding it).
pub const MAX_ROWS: u32 = 250_000;

/// CSV with the delimiter (`,` `;` tab or `|`) and the header row detected. Each data row is
/// a record read "Registro n:" followed by "Coluna: valor" lines (empty cells left out);
/// locations are 1-based data rows, the header not counted.
#[derive(Debug, Clone, Copy, Default)]
pub struct CsvDocumentParser;

impl CsvDocumentParser {
    pub fn parse_bytes(bytes: &[u8]) -> Result<ParsedDocument, ParseError> {
        collect(CsvStream::from_bytes_with_limits(
            bytes,
            CsvLimits {
                max_bytes: crate::MAX_BYTES,
                max_rows: MAX_ROWS,
            },
        )?)
    }

    fn parse_path(path: &Path) -> Result<ParsedDocument, ParseError> {
        collect(CsvStream::open_with_limits(
            path,
            CsvLimits {
                max_rows: MAX_ROWS,
                ..CsvLimits::default()
            },
        )?)
    }
}

/// Reads the stream to the end into a document.
fn collect(mut stream: CsvStream) -> Result<ParsedDocument, ParseError> {
    let blocks = stream.by_ref().collect::<Result<Vec<_>, _>>()?;
    if blocks.is_empty() {
        return Err(ParseError::Empty);
    }
    let metadata = DocumentMetadata {
        dataset: Some(stream.dataset().clone()),
        ..DocumentMetadata::default()
    };
    flat_document(KIND, metadata, blocks, stream.warnings().to_vec())
}

impl DocumentParser for CsvDocumentParser {
    fn document_type(&self) -> DocumentType {
        KIND
    }

    fn version(&self) -> u32 {
        VERSION
    }

    fn parse<'a>(
        &'a self,
        source: &'a DocumentSource,
    ) -> BoxFuture<'a, Result<ParsedDocument, ParseError>> {
        Box::pin(async move {
            if source
                .document_type
                .is_some_and(|declared| declared != KIND)
            {
                return Err(ParseError::Unsupported);
            }
            let path = source.path.clone();
            tokio::task::spawn_blocking(move || Self::parse_path(&path))
                .await
                .map_err(|_| ParseError::Engine("a leitura foi interrompida".into()))?
        })
    }
}

#[cfg(test)]
mod tests {
    use nlmx_domain::{
        parsed::{ContentKind, ParseWarning},
        source::SourceLocation,
    };

    use super::*;

    fn parse(text: &str) -> ParsedDocument {
        CsvDocumentParser::parse_bytes(text.as_bytes()).unwrap()
    }

    fn texts(parsed: &ParsedDocument) -> Vec<&str> {
        parsed.blocks().map(|b| b.text.as_str()).collect()
    }

    #[test]
    fn rows_become_records_with_their_column_names() {
        let parsed = parse("produto;quantidade;preço\nCafé;3;12,50\nChá;;8,00\n");
        assert_eq!(
            texts(&parsed),
            [
                "Registro 1:\nproduto: Café\nquantidade: 3\npreço: 12,50",
                "Registro 2:\nproduto: Chá\npreço: 8,00",
            ]
        );
        assert!(parsed.warnings().is_empty());
        match &parsed.blocks().next().unwrap().kind {
            ContentKind::Record { fields } => {
                let names: Vec<_> = fields.iter().map(|f| f.name.as_str()).collect();
                assert_eq!(names, ["produto", "quantidade", "preço"]);
            }
            other => panic!("{other:?}"),
        }
        // The empty cell is kept in the structure, not in the text.
        match &parsed.blocks().nth(1).unwrap().kind {
            ContentKind::Record { fields } => assert_eq!(fields[1].value, ""),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_metadata_carries_the_dataset() {
        let parsed = parse("a;b\nx;1\ny;2\nz;3\n");
        let dataset = parsed.metadata().dataset.as_ref().expect("dataset");
        assert_eq!(dataset.delimiter, ';');
        assert!(dataset.has_header);
        assert_eq!(dataset.column_names(), ["a", "b"]);
        assert_eq!(dataset.row_count, Some(3));
    }

    #[test]
    fn row_numbers_count_data_rows_without_the_header() {
        let parsed = parse("a,b\n1,x\n2,y\n3,z\n");
        let rows: Vec<_> = parsed.blocks().map(|b| b.location.clone()).collect();
        assert_eq!(
            rows,
            [
                SourceLocation::csv(1, 1).unwrap(),
                SourceLocation::csv(2, 2).unwrap(),
                SourceLocation::csv(3, 3).unwrap(),
            ]
        );
        assert_eq!(parsed.sections().len(), 1);
        assert_eq!(parsed.sections()[0].title, None);
    }

    #[test]
    fn the_delimiter_is_detected() {
        for (delimiter, name) in [
            (",", "vírgula"),
            (";", "ponto e vírgula"),
            ("\t", "tab"),
            ("|", "barra"),
        ] {
            let text = format!("nome{d}valor\n{name}{d}1\noutro{d}2\n", d = delimiter);
            let parsed = parse(&text);
            assert_eq!(
                texts(&parsed)[0],
                format!("Registro 1:\nnome: {name}\nvalor: 1")
            );
        }
    }

    #[test]
    fn decimal_commas_do_not_hide_a_semicolon_delimiter() {
        let parsed = parse("item;preço\nA;1,5\nB;2,5\n");
        assert_eq!(
            texts(&parsed),
            [
                "Registro 1:\nitem: A\npreço: 1,5",
                "Registro 2:\nitem: B\npreço: 2,5"
            ]
        );
    }

    #[test]
    fn quoted_fields_may_hold_delimiters_and_newlines() {
        let parsed = parse("id,nota\n1,\"linha um\nlinha dois, com vírgula\"\n2,ok\n");
        assert_eq!(
            texts(&parsed),
            [
                "Registro 1:\nid: 1\nnota: linha um linha dois, com vírgula",
                "Registro 2:\nid: 2\nnota: ok"
            ]
        );
        assert_eq!(parsed.blocks().count(), 2);
    }

    #[test]
    fn a_file_without_header_gets_positional_names() {
        let parsed = parse("Café,3,12.50\nChá,1,8.00\n");
        assert_eq!(
            texts(&parsed),
            [
                "Registro 1:\ncoluna 1: Café\ncoluna 2: 3\ncoluna 3: 12.50",
                "Registro 2:\ncoluna 1: Chá\ncoluna 2: 1\ncoluna 3: 8.00"
            ]
        );
        assert_eq!(parsed.warnings(), [ParseWarning::NoHeaderRow]);
        assert!(!parsed.metadata().dataset.as_ref().unwrap().has_header);
        // Without a header the first row is data row 1.
        assert_eq!(
            parsed.blocks().next().unwrap().location,
            SourceLocation::csv(1, 1).unwrap()
        );
    }

    #[test]
    fn ragged_rows_are_tolerated() {
        let parsed = parse("a,b,c\n1,2\n3,4,5,6\n");
        assert_eq!(
            texts(&parsed),
            [
                "Registro 1:\na: 1\nb: 2",
                "Registro 2:\na: 3\nb: 4\nc: 5\ncoluna 4: 6"
            ]
        );
    }

    #[test]
    fn rows_of_empty_cells_are_skipped_but_keep_the_numbering() {
        let parsed = parse("a,b\n1,2\n,\n3,4\n");
        let rows: Vec<_> = parsed.blocks().map(|b| b.location.clone()).collect();
        assert_eq!(
            rows,
            [
                SourceLocation::csv(1, 1).unwrap(),
                SourceLocation::csv(3, 3).unwrap()
            ]
        );
    }

    #[test]
    fn utf16_and_legacy_encodings_are_read() {
        let units: Vec<u16> = "nome;valor\nAção;1\n".encode_utf16().collect();
        let mut le = vec![0xFF, 0xFE];
        le.extend(units.iter().flat_map(|u| u.to_le_bytes()));
        let parsed = CsvDocumentParser::parse_bytes(&le).unwrap();
        assert_eq!(texts(&parsed), ["Registro 1:\nnome: Ação\nvalor: 1"]);

        let parsed = CsvDocumentParser::parse_bytes(b"nome;valor\nA\xE7\xE3o;1\n").unwrap();
        assert_eq!(texts(&parsed), ["Registro 1:\nnome: Ação\nvalor: 1"]);
        assert_eq!(parsed.warnings(), [ParseWarning::FallbackEncoding]);
    }

    #[test]
    fn empty_and_binary_files_fail() {
        assert_eq!(
            CsvDocumentParser::parse_bytes(b" \n"),
            Err(ParseError::Empty)
        );
        assert_eq!(
            CsvDocumentParser::parse_bytes(b",,\n,,\n"),
            Err(ParseError::Empty)
        );
        assert_eq!(
            CsvDocumentParser::parse_bytes(b"a,b\n\0,1\n"),
            Err(ParseError::Invalid(DocumentType::Csv))
        );
    }

    #[test]
    fn a_single_row_is_data_not_a_header() {
        // With nothing below it there is no way to tell a header from data: keep the text.
        let parsed = parse("nome,valor\n");
        assert_eq!(
            texts(&parsed),
            ["Registro 1:\ncoluna 1: nome\ncoluna 2: valor"]
        );
        assert_eq!(parsed.warnings(), [ParseWarning::NoHeaderRow]);
    }

    #[test]
    fn too_many_rows_are_refused() {
        let mut text = String::from("a,b\n");
        for _ in 0..=MAX_ROWS {
            text.push_str("x,y\n");
        }
        assert_eq!(
            CsvDocumentParser::parse_bytes(text.as_bytes()),
            Err(ParseError::TooLarge)
        );
    }
}
