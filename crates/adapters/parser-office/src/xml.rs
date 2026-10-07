//! XML reading helpers shared by both formats, and the document properties part.

use nlmx_domain::parsed::DocumentMetadata;
use quick_xml::{
    Reader,
    events::{BytesRef, BytesStart, Event},
};

/// A reader that reports `<a/>` as a start and an end.
pub(crate) fn reader(bytes: &[u8]) -> Reader<&[u8]> {
    let mut reader = Reader::from_reader(bytes);
    let config = reader.config_mut();
    config.check_end_names = false;
    config.expand_empty_elements = true;
    reader
}

/// The XML of a part as UTF-8: a UTF-16 part (byte order mark FF FE / FE FF) is transcoded and a
/// UTF-8 byte order mark is dropped. `None` when a UTF-16 part is not valid UTF-16.
pub(crate) fn to_utf8(bytes: Vec<u8>) -> Option<Vec<u8>> {
    let utf16 = |rest: &[u8], little: bool| -> Option<Vec<u8>> {
        if rest.len() % 2 != 0 {
            return None;
        }
        let units = rest.chunks_exact(2).map(|pair| {
            if little {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        });
        let text: String = char::decode_utf16(units)
            .collect::<Result<String, _>>()
            .ok()?;
        Some(text.into_bytes())
    };
    match bytes.as_slice() {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => Some(rest.to_vec()),
        _ => Some(bytes),
    }
}

/// The lowercase local name of an element or attribute (`w:pStyle` → `pstyle`).
pub(crate) fn lower(local: &[u8]) -> String {
    String::from_utf8_lossy(local).to_ascii_lowercase()
}

/// The value of the attribute whose local name is `local` (case-insensitive, prefix ignored).
pub(crate) fn attr(element: &BytesStart<'_>, local: &str) -> Option<String> {
    element
        .attributes()
        .with_checks(false)
        .filter_map(Result::ok)
        .find(|a| lower(a.key.local_name().as_ref()) == local)
        .map(|a| {
            String::from_utf8_lossy(&a.value)
                .replace("&amp;", "&")
                .to_string()
        })
}

/// The text a character or entity reference stands for; `None` for an unknown entity.
pub(crate) fn entity(reference: &BytesRef<'_>) -> Option<String> {
    if let Ok(Some(c)) = reference.resolve_char_ref() {
        return Some(c.to_string());
    }
    let name = reference.decode().ok()?;
    let predefined = match name.as_ref() {
        "lt" => "<",
        "gt" => ">",
        "amp" => "&",
        "quot" => "\"",
        "apos" => "'",
        _ => return None,
    };
    Some(predefined.to_string())
}

/// Whitespace runs (including non-breaking spaces) become one space; the ends are trimmed.
pub(crate) fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Title, author, language and dates from `docProps/core.xml`.
pub(crate) fn core_properties(bytes: &[u8]) -> DocumentMetadata {
    let mut metadata = DocumentMetadata::default();
    let mut reader = reader(bytes);
    let mut field: Option<String> = None;
    let mut buffer = String::new();
    while let Ok(event) = reader.read_event() {
        match event {
            Event::Start(e) => {
                field = Some(lower(e.local_name().as_ref()));
                buffer.clear();
            }
            Event::Text(t) => {
                if let Ok(text) = t.decode() {
                    buffer.push_str(&text);
                }
            }
            Event::GeneralRef(r) => buffer.push_str(&entity(&r).unwrap_or_default()),
            Event::End(_) => {
                let text = collapse(&buffer);
                if let Some(name) = field.take().filter(|_| !text.is_empty()) {
                    let slot = match name.as_str() {
                        "title" => &mut metadata.title,
                        "creator" => &mut metadata.author,
                        "language" => &mut metadata.language,
                        "created" => &mut metadata.created_at,
                        "modified" => &mut metadata.modified_at,
                        "subject" => &mut metadata.subject,
                        _ => continue,
                    };
                    slot.get_or_insert(text);
                }
                buffer.clear();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    metadata
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_are_resolved() {
        let mut reader = reader(b"<p>a &amp; b &lt;c&gt; &#233; &#x41;</p>");
        let mut out = String::new();
        loop {
            match reader.read_event().unwrap() {
                Event::Text(t) => out.push_str(&t.decode().unwrap()),
                Event::GeneralRef(r) => out.push_str(&entity(&r).unwrap_or_default()),
                Event::Eof => break,
                _ => {}
            }
        }
        assert_eq!(out, "a & b <c> é A");
    }

    #[test]
    fn utf16_and_bom_parts_are_read_as_utf8() {
        let text = "<a>Olá, açaí</a>";
        let mut le = vec![0xFF, 0xFE];
        le.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let mut be = vec![0xFE, 0xFF];
        be.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend(text.as_bytes());
        for bytes in [le, be, bom, text.as_bytes().to_vec()] {
            assert_eq!(to_utf8(bytes).unwrap(), text.as_bytes());
        }
        assert_eq!(to_utf8(vec![0xFF, 0xFE, 0x41]), None, "odd length");
        assert_eq!(
            to_utf8(vec![0xFF, 0xFE, 0x00, 0xD8]),
            None,
            "lone surrogate"
        );
    }

    #[test]
    fn core_properties_are_read() {
        let xml = r#"<cp:coreProperties xmlns:cp="a" xmlns:dc="b" xmlns:dcterms="c">
            <dc:title>Contrato &amp; Anexos</dc:title><dc:creator>Ana</dc:creator>
            <dcterms:created>2024-03-01T10:00:00Z</dcterms:created></cp:coreProperties>"#;
        let m = core_properties(xml.as_bytes());
        assert_eq!(m.title.as_deref(), Some("Contrato & Anexos"));
        assert_eq!(m.author.as_deref(), Some("Ana"));
        assert_eq!(m.created_at.as_deref(), Some("2024-03-01T10:00:00Z"));
        assert_eq!(m.language, None);
    }
}
