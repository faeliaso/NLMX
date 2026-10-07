//! What a CSV file is, decided from its beginning: the encoding, the delimiter, whether the
//! first row is a header, and what kind of value each column holds.

use std::{collections::HashMap, io::Read};

use csv::ReaderBuilder;
use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};
use nlmx_domain::{document_type::DocumentType, parsed::ColumnKind, parsed::ParseError};

const KIND: DocumentType = DocumentType::Csv;

/// Records looked at to detect the delimiter and the header.
pub(crate) const SAMPLE_RECORDS: usize = 50;
/// Decoded bytes read from the start of the file for detection.
pub(crate) const SAMPLE_BYTES: u64 = 256 * 1024;
pub(crate) const DELIMITERS: [u8; 4] = *b",;\t|";
/// Header cells longer than this (in characters) look like data, not names.
const MAX_HEADER_CELL_CHARS: usize = 40;
const MAX_HEADER_CELL_WORDS: usize = 4;
/// Share of a column's non-empty cells that must be of one kind for the column to have it.
const KIND_SHARE_PERCENT: usize = 80;

pub(crate) struct Charset {
    pub encoding: &'static Encoding,
    /// The bytes were not UTF-8 and are read as Windows-1252.
    pub fallback: bool,
}

/// Reads until `buf` is full or the input ends; returns the bytes read.
fn fill(reader: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

/// The encoding of a text stream, in one pass and constant memory: a UTF-16 BOM decides;
/// otherwise strict UTF-8 (with or without BOM), then Windows-1252. A NUL byte in a stream that
/// is not UTF-16 means binary data (`Invalid`).
pub(crate) fn detect_charset(mut reader: impl Read) -> Result<Charset, ParseError> {
    let io_error = |_| ParseError::Engine("não foi possível ler o arquivo".into());
    let mut buf = vec![0u8; 64 * 1024];
    let mut pending: Vec<u8> = Vec::new();
    let mut utf8_ok = true;
    let mut utf8_bom = false;
    let mut first = true;
    loop {
        let n = fill(&mut reader, &mut buf).map_err(io_error)?;
        if n == 0 {
            break;
        }
        let mut chunk = &buf[..n];
        if first {
            first = false;
            if chunk.starts_with(&[0xFF, 0xFE]) {
                return Ok(Charset {
                    encoding: UTF_16LE,
                    fallback: false,
                });
            }
            if chunk.starts_with(&[0xFE, 0xFF]) {
                return Ok(Charset {
                    encoding: UTF_16BE,
                    fallback: false,
                });
            }
            if chunk.starts_with(&[0xEF, 0xBB, 0xBF]) {
                utf8_bom = true;
                chunk = &chunk[3..];
            }
        }
        if chunk.contains(&0) {
            return Err(ParseError::Invalid(KIND));
        }
        if utf8_ok {
            pending.extend_from_slice(chunk);
            match std::str::from_utf8(&pending) {
                Ok(_) => pending.clear(),
                Err(error) if error.error_len().is_none() => {
                    // A character cut by the end of the chunk: keep its first bytes.
                    pending.drain(..error.valid_up_to());
                }
                Err(_) => {
                    utf8_ok = false;
                    pending.clear();
                }
            }
        }
    }
    if !pending.is_empty() {
        utf8_ok = false;
    }
    if utf8_ok {
        Ok(Charset {
            encoding: UTF_8,
            fallback: false,
        })
    } else if utf8_bom {
        Err(ParseError::Encoding)
    } else {
        Ok(Charset {
            encoding: WINDOWS_1252,
            fallback: true,
        })
    }
}

pub(crate) fn reader(text: &str, delimiter: u8) -> csv::Reader<&[u8]> {
    ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes())
}

/// The candidate whose first records have the most consistent number of columns (at least
/// two); on a tie, the one with more columns, then the earlier candidate. Quoted fields do not
/// count their delimiters.
pub(crate) fn detect_delimiter(text: &str) -> u8 {
    let mut best: Option<((usize, usize), u8)> = None;
    for delimiter in DELIMITERS {
        let widths: Vec<usize> = reader(text, delimiter)
            .records()
            .take(SAMPLE_RECORDS)
            .map_while(Result::ok)
            .map(|record| record.len())
            .collect();
        if widths.is_empty() {
            continue;
        }
        let mut counts: HashMap<usize, usize> = HashMap::new();
        for width in &widths {
            *counts.entry(*width).or_default() += 1;
        }
        let (modal, consistent) = counts
            .into_iter()
            .max_by_key(|(width, count)| (*count, *width))
            .unwrap_or((1, 0));
        if modal < 2 {
            continue;
        }
        let key = (consistent * 10_000 / widths.len(), modal);
        if best.is_none_or(|(current, _)| key > current) {
            best = Some((key, delimiter));
        }
    }
    best.map_or(b',', |(_, delimiter)| delimiter)
}

/// Up to [`SAMPLE_RECORDS`] records of `text`, cells trimmed.
pub(crate) fn sample_records(text: &str, delimiter: u8) -> Vec<Vec<String>> {
    reader(text, delimiter)
        .records()
        .take(SAMPLE_RECORDS)
        .map_while(Result::ok)
        .map(|record| record.iter().map(|c| c.trim().to_string()).collect())
        .collect()
}

/// How sure the header decision is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Header {
    /// The first row is a header and the data confirms it (a text name above values of
    /// another kind).
    Confident,
    /// Every cell is text, so only the shape of the first row suggests a header.
    Uncertain,
    /// The first row is data.
    Absent,
}

pub(crate) fn is_number(cell: &str) -> bool {
    let cleaned: String = cell.replace("R$", "").replace(['%', ' ', '\u{a0}'], "");
    let Some(first) = cleaned.chars().next() else {
        return false;
    };
    if !(first.is_ascii_digit() || matches!(first, '-' | '+' | '.' | ',')) {
        return false;
    }
    let normalized = if cleaned.contains('.') && cleaned.contains(',') {
        cleaned.replace('.', "").replace(',', ".")
    } else {
        cleaned.replace(',', ".")
    };
    normalized.parse::<f64>().is_ok()
}

/// ISO (`2024-03-15`, optionally with a time) and day-first (`15/03/2024`, `15-03-2024`,
/// `15.03.2024`) dates.
pub(crate) fn is_date(cell: &str) -> bool {
    let (date, time) = match cell.split_once(['T', ' ']) {
        Some((date, time)) => (date, Some(time)),
        None => (cell, None),
    };
    if let Some(time) = time {
        let time = time.trim_end_matches('Z');
        let parts: Vec<&str> = time.split(':').collect();
        let digits = |s: &str, max: usize| {
            !s.is_empty() && s.len() <= max && s.chars().all(|c| c.is_ascii_digit())
        };
        let valid = matches!(parts.len(), 2 | 3)
            && digits(parts[0], 2)
            && digits(parts[1], 2)
            && parts.get(2).is_none_or(|s| {
                s.split('.')
                    .next()
                    .is_some_and(|seconds| digits(seconds, 2))
            });
        if !valid {
            return false;
        }
    }
    let Some(separator) = date.chars().find(|c| matches!(c, '-' | '/' | '.')) else {
        return false;
    };
    let parts: Vec<&str> = date.split(separator).collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()))
    {
        return false;
    }
    let number = |s: &str| s.parse::<u32>().unwrap_or(0);
    let (year_first, day_first) = (parts[0].len() == 4, parts[2].len() == 4);
    let valid_month_day =
        |month: u32, day: u32| (1..=12).contains(&month) && (1..=31).contains(&day);
    (year_first
        && parts[1].len() <= 2
        && parts[2].len() <= 2
        && valid_month_day(number(parts[1]), number(parts[2])))
        || (day_first
            && parts[0].len() <= 2
            && parts[1].len() <= 2
            && valid_month_day(number(parts[1]), number(parts[0])))
}

pub(crate) fn is_boolean(cell: &str) -> bool {
    matches!(
        cell.to_lowercase().as_str(),
        "true" | "false" | "sim" | "não" | "nao" | "verdadeiro" | "falso" | "yes" | "no"
    )
}

/// The kind of one non-empty cell (`Text` when it is none of the others).
pub(crate) fn cell_kind(cell: &str) -> ColumnKind {
    if is_number(cell) {
        ColumnKind::Number
    } else if is_date(cell) {
        ColumnKind::Date
    } else if is_boolean(cell) {
        ColumnKind::Boolean
    } else {
        ColumnKind::Text
    }
}

/// The kind of each of `width` columns: the one at least 80% of its non-empty cells have, else
/// `Text` (also for a column with no values).
pub(crate) fn column_kinds(rows: &[Vec<String>], width: usize) -> Vec<ColumnKind> {
    (0..width)
        .map(|column| {
            let mut counts = [0usize; 3];
            let mut total = 0usize;
            for cell in rows
                .iter()
                .filter_map(|row| row.get(column))
                .filter(|c| !c.is_empty())
            {
                total += 1;
                match cell_kind(cell) {
                    ColumnKind::Number => counts[0] += 1,
                    ColumnKind::Date => counts[1] += 1,
                    ColumnKind::Boolean => counts[2] += 1,
                    ColumnKind::Text => {}
                }
            }
            let kinds = [ColumnKind::Number, ColumnKind::Date, ColumnKind::Boolean];
            let (count, kind) = counts
                .into_iter()
                .zip(kinds)
                .max_by_key(|(count, _)| *count)
                .unwrap_or((0, ColumnKind::Text));
            if total > 0 && count * 100 >= total * KIND_SHARE_PERCENT {
                kind
            } else {
                ColumnKind::Text
            }
        })
        .collect()
}

/// Whether the first record is a header, never assumed:
///
/// - with fewer than two records there is nothing to compare: data;
/// - a number, date or boolean in the first row means data; so do repeated names and an
///   all-empty row; an empty cell is allowed only above a column of numbers (a row index);
/// - **confident** when some column has a text name above values of another kind;
/// - **uncertain** when the whole table is text: a header only if there are two or more
///   columns, each name short (≤ 40 characters, ≤ 4 words) and not repeated below it.
pub(crate) fn decide_header(records: &[Vec<String>]) -> Header {
    let [first, rest @ ..] = records else {
        return Header::Absent;
    };
    if rest.is_empty() || first.iter().all(|cell| cell.is_empty()) {
        return Header::Absent;
    }
    let kinds = column_kinds(rest, first.len());
    let mut seen: Vec<String> = Vec::new();
    for (column, cell) in first.iter().enumerate() {
        if cell.is_empty() {
            if kinds[column] != ColumnKind::Number {
                return Header::Absent;
            }
            continue;
        }
        if cell_kind(cell) != ColumnKind::Text {
            return Header::Absent;
        }
        let key = cell.to_lowercase();
        if seen.contains(&key) {
            return Header::Absent;
        }
        seen.push(key);
    }
    let contrast = first
        .iter()
        .enumerate()
        .any(|(column, cell)| !cell.is_empty() && kinds[column] != ColumnKind::Text);
    if contrast {
        return Header::Confident;
    }
    let looks_like_names = first.len() >= 2
        && first.iter().enumerate().all(|(column, cell)| {
            !cell.is_empty()
                && cell.chars().count() <= MAX_HEADER_CELL_CHARS
                && cell.split_whitespace().count() <= MAX_HEADER_CELL_WORDS
                && !rest.iter().any(|row| {
                    row.get(column).is_some_and(|below| {
                        below.eq_ignore_ascii_case(cell)
                            || below.to_lowercase() == cell.to_lowercase()
                    })
                })
        });
    if looks_like_names {
        Header::Uncertain
    } else {
        Header::Absent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cells: &[&str]) -> Vec<String> {
        cells.iter().map(|c| c.to_string()).collect()
    }

    #[test]
    fn numbers_are_recognized_in_brazilian_and_plain_formats() {
        for number in ["12", "-3,5", "1.234,56", "R$ 10,00", "45%", "0.5", ".5"] {
            assert!(is_number(number), "{number}");
        }
        for text in ["", "abc", "12a", "inf", "NaN", "2024-01-02", "1,2,3"] {
            assert!(!is_number(text), "{text}");
        }
    }

    #[test]
    fn dates_and_booleans_are_recognized() {
        for date in [
            "2024-03-15",
            "15/03/2024",
            "15-03-2024",
            "15.03.2024",
            "2024-03-15T10:30:00Z",
            "2024-03-15 10:30",
            "5/3/2024",
        ] {
            assert!(is_date(date), "{date}");
        }
        for text in [
            "",
            "2024",
            "2024-13-01",
            "32/01/2024",
            "12/05",
            "a-b-c",
            "2024-03-15 25h",
            "1.5.2",
        ] {
            assert!(!is_date(text), "{text}");
        }
        for value in ["true", "FALSE", "Sim", "não", "nao", "yes", "No"] {
            assert!(is_boolean(value), "{value}");
        }
        assert!(!is_boolean("talvez"));
        assert_eq!(cell_kind("12,5"), ColumnKind::Number);
        assert_eq!(cell_kind("2024-01-02"), ColumnKind::Date);
        assert_eq!(cell_kind("sim"), ColumnKind::Boolean);
        assert_eq!(cell_kind("Recife"), ColumnKind::Text);
    }

    #[test]
    fn a_column_has_the_kind_most_of_its_cells_have() {
        let rows = vec![
            row(&["a", "1", "x", "", "sim"]),
            row(&["b", "2", "y", "", "não"]),
            row(&["c", "3", "5", "", "sim"]),
            row(&["d", "4", "z", "", "talvez"]),
            row(&["e", "oops", "w", "", "sim"]),
        ];
        assert_eq!(
            column_kinds(&rows, 5),
            [
                ColumnKind::Text,
                ColumnKind::Number, // 4 of 5 = 80%
                ColumnKind::Text,
                ColumnKind::Text, // no values
                ColumnKind::Boolean,
            ]
        );
        // A column wider than every row is just empty.
        assert_eq!(column_kinds(&rows, 6)[5], ColumnKind::Text);
    }

    #[test]
    fn a_text_name_above_other_kinds_is_a_confident_header() {
        let data = row(&["Ana", "31"]);
        assert_eq!(
            decide_header(&[row(&["nome", "idade"]), data.clone(), row(&["Bia", "29"])]),
            Header::Confident
        );
        // A row index: the empty first cell is fine above numbers.
        assert_eq!(
            decide_header(&[
                row(&["", "a", "b"]),
                row(&["0", "x", "1"]),
                row(&["1", "y", "2"])
            ]),
            Header::Confident
        );
    }

    #[test]
    fn a_first_row_of_data_is_not_a_header() {
        let data = row(&["x", "1"]);
        assert_eq!(
            decide_header(&[row(&["nome", "2024"]), data.clone()]),
            Header::Absent,
            "a number"
        );
        assert_eq!(
            decide_header(&[row(&["nome", "2024-01-02"]), data.clone()]),
            Header::Absent,
            "a date"
        );
        assert_eq!(
            decide_header(&[row(&["nome", ""]), data.clone()]),
            Header::Absent,
            "an empty cell above text"
        );
        assert_eq!(
            decide_header(&[row(&["a", "A"]), data.clone()]),
            Header::Absent,
            "repeated names"
        );
        assert_eq!(decide_header(&[row(&["nome", "valor"])]), Header::Absent);
        assert_eq!(decide_header(&[]), Header::Absent);
        assert_eq!(
            decide_header(&[row(&["", ""]), data]),
            Header::Absent,
            "empty row"
        );
        assert_eq!(
            decide_header(&[
                row(&["Ana", "31", "2020-03-15"]),
                row(&["Bruno", "45", "2019-11-02"]),
            ]),
            Header::Absent,
            "names, ages and dates"
        );
    }

    #[test]
    fn an_all_text_table_is_only_an_uncertain_header() {
        assert_eq!(
            decide_header(&[
                row(&["nome", "cidade"]),
                row(&["João", "Fortaleza"]),
                row(&["Maria", "Recife"]),
            ]),
            Header::Uncertain
        );
        // A name that also appears below it is data.
        assert_eq!(
            decide_header(&[
                row(&["Recife", "PE"]),
                row(&["Natal", "RN"]),
                row(&["recife", "PE"]),
            ]),
            Header::Absent
        );
        // One column of text, or long cells: no shape to go by.
        assert_eq!(
            decide_header(&[row(&["nomes"]), row(&["Ana"]), row(&["Bia"])]),
            Header::Absent
        );
        assert_eq!(
            decide_header(&[
                row(&[
                    "uma frase longa demais para ser um nome de coluna de verdade",
                    "b"
                ]),
                row(&["x", "y"]),
            ]),
            Header::Absent
        );
    }

    #[test]
    fn the_delimiter_is_detected() {
        for (delimiter, expected) in [(",", b','), (";", b';'), ("\t", b'\t'), ("|", b'|')] {
            let text = format!("nome{d}valor\nA{d}1\nB{d}2\n", d = delimiter);
            assert_eq!(detect_delimiter(&text), expected, "{delimiter:?}");
        }
        // Decimal commas do not hide a semicolon; quoted delimiters do not count.
        assert_eq!(detect_delimiter("item;preço\nA;1,5\nB;2,5\n"), b';');
        assert_eq!(detect_delimiter("a,b\n\"x;y;z\",1\n\"p;q\",2\n"), b',');
        // Nothing to go by: the comma.
        assert_eq!(detect_delimiter("uma coluna só\nsegunda linha\n"), b',');
    }

    #[test]
    fn the_charset_is_detected_without_reading_everything_into_memory() {
        let detect = |bytes: &[u8]| detect_charset(std::io::Cursor::new(bytes.to_vec()));
        let utf8 = detect("ação".as_bytes()).unwrap();
        assert_eq!((utf8.encoding, utf8.fallback), (UTF_8, false));
        let legacy = detect(b"A\xE7\xE3o").unwrap();
        assert_eq!((legacy.encoding, legacy.fallback), (WINDOWS_1252, true));
        assert_eq!(
            detect(&[0xFF, 0xFE, 0x41, 0x00]).unwrap().encoding,
            UTF_16LE
        );
        assert_eq!(
            detect(&[0xFE, 0xFF, 0x00, 0x41]).unwrap().encoding,
            UTF_16BE
        );
        assert_eq!(detect(b"").unwrap().encoding, UTF_8);
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend("ação".as_bytes());
        assert_eq!(detect(&with_bom).unwrap().encoding, UTF_8);
        assert_eq!(
            detect(&[0xEF, 0xBB, 0xBF, 0xFF, 0x41]).err(),
            Some(ParseError::Encoding)
        );
        assert_eq!(detect(b"a\0b").err(), Some(ParseError::Invalid(KIND)));
    }

    #[test]
    fn a_multibyte_character_split_across_chunks_is_still_utf8() {
        // 64 KiB chunks: put "ã" (2 bytes) astride the boundary.
        let mut bytes = vec![b'a'; 64 * 1024 - 1];
        bytes.extend("ã".as_bytes());
        bytes.extend(b"bc");
        let charset = detect_charset(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!((charset.encoding, charset.fallback), (UTF_8, false));
        // A NUL in a later chunk is found too.
        let mut late = vec![b'a'; 200 * 1024];
        late.push(0);
        assert_eq!(
            detect_charset(std::io::Cursor::new(late)).err(),
            Some(ParseError::Invalid(KIND))
        );
    }
}
