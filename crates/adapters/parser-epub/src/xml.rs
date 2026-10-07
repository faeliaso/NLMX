//! Lenient XML/XHTML reading helpers: tolerant of mismatched end tags, HTML entities and
//! unknown prefixes, which real-world EPUBs are full of.

use quick_xml::{
    Reader,
    escape::resolve_html5_entity,
    events::{BytesRef, BytesStart},
};

/// A reader that tolerates mismatched end tags and reports `<a/>` as a start and an end.
pub(crate) fn reader(bytes: &[u8]) -> Reader<&[u8]> {
    let mut reader = Reader::from_reader(bytes);
    let config = reader.config_mut();
    config.check_end_names = false;
    config.expand_empty_elements = true;
    reader
}

/// The lowercase local name of an element or attribute (`dc:title` → `title`).
pub(crate) fn lower(local: &[u8]) -> String {
    String::from_utf8_lossy(local).to_ascii_lowercase()
}

/// The value of the attribute whose local name is `local` (`epub:type` → `type`).
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
        other => return resolve_html5_entity(other).map(str::to_string),
    };
    Some(predefined.to_string())
}

/// Whitespace runs (including non-breaking spaces) become one space; the ends are trimmed.
pub(crate) fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use quick_xml::events::Event;

    use super::*;

    fn text_of(xml: &str) -> String {
        let mut reader = reader(xml.as_bytes());
        let mut out = String::new();
        loop {
            match reader.read_event().unwrap() {
                Event::Text(t) => out.push_str(&t.decode().unwrap()),
                Event::GeneralRef(r) => out.push_str(&entity(&r).unwrap_or_default()),
                Event::Eof => return out,
                _ => {}
            }
        }
    }

    #[test]
    fn entities_are_resolved() {
        assert_eq!(
            text_of("<p>a&nbsp;b &eacute; &amp; &lt;x&gt; &#233; &#x41;</p>"),
            "a\u{a0}b é & <x> é A"
        );
        assert_eq!(text_of("<p>x&unknownthing;y</p>"), "xy");
    }

    #[test]
    fn mismatched_end_tags_are_tolerated() {
        assert_eq!(text_of("<p><b>um</i> dois</p>"), "um dois");
    }

    #[test]
    fn whitespace_collapses_including_non_breaking_spaces() {
        assert_eq!(collapse("  a \n\t b\u{a0}\u{a0}c  "), "a b c");
        assert_eq!(collapse(" \n "), "");
    }

    #[test]
    fn attributes_match_by_local_name() {
        let xml = r#"<nav epub:type="toc" ID="x"/>"#;
        let mut reader = reader(xml.as_bytes());
        let Event::Start(start) = reader.read_event().unwrap() else {
            panic!("a start tag");
        };
        assert_eq!(attr(&start, "type").as_deref(), Some("toc"));
        assert_eq!(attr(&start, "id").as_deref(), Some("x"));
        assert_eq!(attr(&start, "missing"), None);
    }
}
