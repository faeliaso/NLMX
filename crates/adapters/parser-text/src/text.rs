//! TXT: one paragraph per run of non-blank lines, located by character offsets.

use std::ops::Range;

use nlmx_application::ports::{BoxFuture, DocumentParser, DocumentSource};
use nlmx_domain::{
    document_type::DocumentType,
    parsed::{
        ContentBlock, ContentKind, DocumentMetadata, ParseError, ParseWarning, ParsedDocument,
    },
    source::SourceLocation,
};

use crate::{decode::decode, flat_document, lines::paragraph_ranges, parse_file};

const VERSION: u32 = 2;

/// A paragraph longer than this many characters is split at its line breaks.
const LONG_PARAGRAPH: usize = 2000;
/// Target size of the pieces of a split paragraph (a longer single line stays whole).
const PIECE: usize = 1000;
const KIND: DocumentType = DocumentType::Text;

/// Plain text. Locations are `[start, end)` in characters of the decoded text with `\n` line
/// ends (a Windows-1252 or UTF-16 file is counted after decoding). No headings are invented.
/// A paragraph over 2000 characters (text without blank lines) is split at its line breaks
/// into pieces of about 1000 characters, so a location never covers a whole large file.
#[derive(Debug, Clone, Copy, Default)]
pub struct TextDocumentParser;

impl TextDocumentParser {
    pub fn parse_bytes(bytes: &[u8]) -> Result<ParsedDocument, ParseError> {
        let decoded = decode(bytes, KIND)?;
        let text = &decoded.text;
        if text.trim().is_empty() {
            return Err(ParseError::Empty);
        }
        let mut blocks = Vec::new();
        let (mut byte, mut chars) = (0, 0u32);
        let pieces = paragraph_ranges(text)
            .into_iter()
            .flat_map(|range| split_long(text, range));
        for range in pieces {
            chars += text[byte..range.start].chars().count() as u32;
            let length = text[range.clone()].chars().count() as u32;
            let location = SourceLocation::text(chars, chars + length)
                .map_err(|_| ParseError::Invalid(KIND))?;
            blocks.push(ContentBlock {
                kind: ContentKind::Paragraph,
                text: text[range.clone()].to_string(),
                location,
            });
            chars += length;
            byte = range.end;
        }
        let warnings = if decoded.fallback {
            vec![ParseWarning::FallbackEncoding]
        } else {
            Vec::new()
        };
        flat_document(KIND, DocumentMetadata::default(), blocks, warnings)
    }
}

/// The byte ranges of a paragraph: itself, or, when it is longer than [`LONG_PARAGRAPH`]
/// characters, runs of whole lines of at most [`PIECE`] characters (a longer line alone).
fn split_long(text: &str, range: Range<usize>) -> Vec<Range<usize>> {
    let slice = &text[range.clone()];
    if slice.chars().count() <= LONG_PARAGRAPH {
        return vec![range];
    }
    let mut pieces = Vec::new();
    let mut current: Option<(Range<usize>, usize)> = None;
    let mut position = range.start;
    for line in slice.split_inclusive('\n') {
        let content = line.trim_end_matches('\n');
        let length = content.chars().count();
        let (start, end) = (position, position + content.len());
        position += line.len();
        if content.trim().is_empty() {
            continue;
        }
        current = match current.take() {
            Some((open, chars)) if chars + 1 + length <= PIECE => {
                Some((open.start..end, chars + 1 + length))
            }
            Some((open, _)) => {
                pieces.push(open);
                Some((start..end, length))
            }
            None => Some((start..end, length)),
        };
    }
    pieces.extend(current.map(|(open, _)| open));
    pieces
        .into_iter()
        .map(|piece| {
            let slice = &text[piece.clone()];
            let start = piece.start + (slice.len() - slice.trim_start().len());
            let end = piece.end - (slice.len() - slice.trim_end().len());
            start..end
        })
        .collect()
}

impl DocumentParser for TextDocumentParser {
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
        parse_file(KIND, source, Self::parse_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> ParsedDocument {
        TextDocumentParser::parse_bytes(text.as_bytes()).unwrap()
    }

    fn ranges(parsed: &ParsedDocument) -> Vec<(String, (u32, u32))> {
        parsed
            .blocks()
            .map(|b| match b.location {
                SourceLocation::Text { start, end } => (b.text.clone(), (start, end)),
                _ => panic!("not a text location"),
            })
            .collect()
    }

    #[test]
    fn paragraphs_keep_their_character_offsets() {
        let parsed = parse("Primeiro\nparágrafo.\n\nSegundo.\n");
        assert_eq!(
            ranges(&parsed),
            [
                ("Primeiro\nparágrafo.".to_string(), (0, 19)),
                ("Segundo.".to_string(), (21, 29)),
            ]
        );
        assert_eq!(parsed.document_type(), DocumentType::Text);
        assert!(parsed.metadata().title.is_none());
        assert_eq!(parsed.sections().len(), 1);
    }

    #[test]
    fn offsets_count_characters_not_bytes() {
        // "ação" is 4 characters and 6 bytes; offsets must not drift on later paragraphs.
        let text = "ação 日本\n\nfim";
        let parsed = parse(text);
        let found = ranges(&parsed);
        assert_eq!(found[0], ("ação 日本".to_string(), (0, 7)));
        assert_eq!(found[1], ("fim".to_string(), (9, 12)));
        let chars: Vec<char> = text.chars().collect();
        let (start, end) = found[1].1;
        assert_eq!(
            chars[start as usize..end as usize]
                .iter()
                .collect::<String>(),
            "fim"
        );
    }

    #[test]
    fn leading_and_trailing_blank_space_is_trimmed() {
        let parsed = parse("\n\n   recuo\n\n");
        assert_eq!(ranges(&parsed), [("recuo".to_string(), (5, 10))]);
    }

    #[test]
    fn windows_line_ends_and_legacy_encoding_are_normalized() {
        let parsed = TextDocumentParser::parse_bytes(b"A\xE7\xE3o\r\n\r\nfim").unwrap();
        assert_eq!(
            ranges(&parsed),
            [("Ação".to_string(), (0, 4)), ("fim".to_string(), (6, 9))]
        );
        assert_eq!(parsed.warnings(), [ParseWarning::FallbackEncoding]);
    }

    #[test]
    fn empty_and_binary_files_fail() {
        assert_eq!(TextDocumentParser::parse_bytes(b""), Err(ParseError::Empty));
        assert_eq!(
            TextDocumentParser::parse_bytes(b" \n\t\n"),
            Err(ParseError::Empty)
        );
        assert_eq!(
            TextDocumentParser::parse_bytes(b"a\0b"),
            Err(ParseError::Invalid(DocumentType::Text))
        );
    }

    #[test]
    fn the_size_limit_is_enforced() {
        let big = vec![b'a'; crate::MAX_BYTES as usize + 1];
        assert_eq!(
            TextDocumentParser::parse_bytes(&big),
            Err(ParseError::TooLarge)
        );
    }

    fn slice(text: &str, start: u32, end: u32) -> String {
        text.chars()
            .skip(start as usize)
            .take((end - start) as usize)
            .collect()
    }

    /// `lines` lines of `width` characters each, as one paragraph.
    fn long_paragraph(lines: usize, width: usize) -> String {
        (0..lines)
            .map(|i| format!("{i:03}{}", "á".repeat(width - 3)))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_long_paragraph_is_split_at_line_breaks_with_exact_offsets() {
        let body = long_paragraph(60, 50); // 3059 characters, no blank line
        let text = format!("Antes.\n\n{body}\n\nDepois.");
        let parsed = parse(&text);
        let found = ranges(&parsed);
        assert!(found.len() > 3, "{}", found.len());
        // Every block is exactly what its offsets point to in the decoded text.
        for (block, (start, end)) in &found {
            assert_eq!(&slice(&text, *start, *end), block);
        }
        // Pieces of the big paragraph stay near the target size.
        for (block, _) in &found[1..found.len() - 1] {
            assert!(block.chars().count() <= PIECE, "{}", block.chars().count());
        }
        // No line is lost or repeated.
        let rebuilt: Vec<_> = found[1..found.len() - 1]
            .iter()
            .flat_map(|(block, _)| block.lines().map(String::from))
            .collect();
        assert_eq!(rebuilt, body.lines().collect::<Vec<_>>());
        assert_eq!(found[0].0, "Antes.");
        assert_eq!(found.last().unwrap().0, "Depois.");
    }

    #[test]
    fn a_single_very_long_line_stays_whole() {
        let line = "x".repeat(2500);
        let parsed = parse(&format!("{line}\ncurta"));
        let found = ranges(&parsed);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, line);
        assert_eq!(found[1].0, "curta");
    }

    #[test]
    fn paragraphs_up_to_the_limit_are_unchanged() {
        let body = long_paragraph(40, 50); // 40 * 50 + 39 = 2039 > limit, so use 39 lines
        assert!(body.chars().count() > LONG_PARAGRAPH);
        let short = long_paragraph(39, 50);
        assert!(short.chars().count() <= LONG_PARAGRAPH);
        let parsed = parse(&short);
        assert_eq!(ranges(&parsed).len(), 1);
        assert_eq!(parsed.blocks().next().unwrap().text, short);
    }

    #[test]
    fn the_parser_claims_only_its_format() {
        let parser = TextDocumentParser;
        assert_eq!(parser.document_type(), DocumentType::Text);
        assert!(parser.supports_mime("text/plain; charset=utf-8"));
        assert!(!parser.supports_mime("text/csv"));
    }
}
