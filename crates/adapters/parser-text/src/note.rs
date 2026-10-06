//! A note: pasted text with no file. It is split into paragraphs exactly like a TXT file (same
//! parser), and located by character offsets in the note's text.

use nlmx_application::ports::{BoxFuture, DocumentParser, DocumentSource};
use nlmx_domain::{
    document_type::DocumentType,
    parsed::{ContentBlock, ParseError, ParsedDocument},
    source::SourceLocation,
};

use crate::{TextDocumentParser, flat_document};

const VERSION: u32 = 1;
const KIND: DocumentType = DocumentType::Note;

#[derive(Debug, Clone, Copy, Default)]
pub struct NoteDocumentParser;

impl NoteDocumentParser {
    pub fn new() -> Self {
        Self
    }

    /// Parses the text of a note. The title is the document's name (stored by the ingestion),
    /// so the parsed metadata has none.
    pub fn parse_text(text: &str) -> Result<ParsedDocument, ParseError> {
        let parsed = TextDocumentParser::parse_bytes(text.as_bytes())?;
        let (_, metadata, _, sections, warnings) = parsed.into_parts();
        let blocks = sections
            .into_iter()
            .flat_map(|section| section.blocks)
            .map(|block| {
                let location = match block.location {
                    SourceLocation::Text { start, end } => {
                        SourceLocation::note(start, end).map_err(|_| ParseError::Invalid(KIND))?
                    }
                    _ => return Err(ParseError::Invalid(KIND)),
                };
                Ok(ContentBlock {
                    kind: block.kind,
                    text: block.text,
                    location,
                })
            })
            .collect::<Result<Vec<_>, ParseError>>()?;
        flat_document(KIND, metadata, blocks, warnings)
    }
}

impl DocumentParser for NoteDocumentParser {
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
            // A note has no file: its text travels in the source and a path is never read.
            let text = source.text.clone().ok_or(ParseError::NotFound)?;
            tokio::task::spawn_blocking(move || Self::parse_text(&text))
                .await
                .map_err(|_| ParseError::Engine("a leitura foi interrompida".into()))?
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_are_located_by_characters_of_the_note() {
        let parsed = NoteDocumentParser::parse_text("Primeiro.\n\nSegundo ação.").unwrap();
        assert_eq!(parsed.document_type(), DocumentType::Note);
        let ranges: Vec<_> = parsed.blocks().map(|b| b.location.clone()).collect();
        assert_eq!(
            ranges,
            [
                SourceLocation::note(0, 9).unwrap(),
                SourceLocation::note(11, 24).unwrap()
            ]
        );
        assert!(parsed.metadata().title.is_none());
    }

    #[tokio::test]
    async fn a_source_without_text_is_not_found_and_a_path_is_never_read() {
        let parser = NoteDocumentParser::new();
        let mut source = DocumentSource::of_type("/etc/hosts", DocumentType::Note);
        assert_eq!(
            parser.parse(&source).await.unwrap_err(),
            ParseError::NotFound
        );
        source.text = Some("texto".into());
        assert!(parser.parse(&source).await.is_ok());
        assert_eq!(
            parser
                .parse(&DocumentSource::of_type("x", DocumentType::Text))
                .await
                .unwrap_err(),
            ParseError::Unsupported
        );
    }
}
