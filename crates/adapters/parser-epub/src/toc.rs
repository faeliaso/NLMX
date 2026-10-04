//! Chapter titles from the table of contents: the EPUB 3 navigation document or the EPUB 2 NCX.

use std::collections::HashMap;

use quick_xml::events::Event;

use crate::{
    archive::resolve,
    xml::{attr, collapse, entity, lower, reader},
};

/// Titles by zip path (fragments dropped); the first entry for a path wins.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Titles(HashMap<String, String>);

impl Titles {
    pub(crate) fn get(&self, path: &str) -> Option<&str> {
        self.0.get(path).map(String::as_str)
    }

    /// Adds the titles of `other` for paths that have none yet.
    pub(crate) fn fill_from(&mut self, other: Titles) {
        for (path, title) in other.0 {
            self.0.entry(path).or_insert(title);
        }
    }

    fn add(&mut self, path: String, title: &str) {
        let title = collapse(title);
        if !title.is_empty() {
            self.0.entry(path).or_insert(title);
        }
    }
}

/// The `<nav epub:type="toc">` of a navigation document (the first `<nav>` if none is typed
/// as a table of contents). `base_dir` is the directory of the navigation document.
pub(crate) fn parse_nav(bytes: &[u8], base_dir: &str) -> Option<Titles> {
    let mut reader = reader(bytes);
    // (is a table of contents, links as (href, text))
    let mut navs: Vec<(bool, Vec<(String, String)>)> = Vec::new();
    let mut in_nav = false;
    let mut link: Option<(String, String)> = None;
    loop {
        match reader.read_event().ok()? {
            Event::Start(e) => match lower(e.local_name().as_ref()).as_str() {
                "nav" => {
                    in_nav = true;
                    let toc =
                        attr(&e, "type").is_some_and(|t| t.split_whitespace().any(|w| w == "toc"));
                    navs.push((toc, Vec::new()));
                }
                "a" if in_nav => {
                    if let Some(href) = attr(&e, "href") {
                        link = Some((href, String::new()));
                    }
                }
                _ => {}
            },
            Event::End(e) => match lower(e.local_name().as_ref()).as_str() {
                "nav" => in_nav = false,
                "a" => {
                    if let (Some(done), Some(nav)) = (link.take(), navs.last_mut()) {
                        nav.1.push(done);
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if let Some((_, text)) = link.as_mut() {
                    text.push_str(&t.decode().ok()?);
                }
            }
            Event::GeneralRef(r) => {
                if let Some((_, text)) = link.as_mut() {
                    text.push_str(&entity(&r).unwrap_or_default());
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let chosen = navs.iter().find(|(toc, _)| *toc).or_else(|| navs.first())?;
    let mut titles = Titles::default();
    for (href, text) in &chosen.1 {
        titles.add(resolve(base_dir, href), text);
    }
    Some(titles)
}

/// The `navLabel`/`content` pairs of an NCX. `base_dir` is the directory of the NCX.
pub(crate) fn parse_ncx(bytes: &[u8], base_dir: &str) -> Option<Titles> {
    let mut reader = reader(bytes);
    let mut titles = Titles::default();
    let mut in_label = false;
    let mut label = String::new();
    let mut pending: Option<String> = None;
    loop {
        match reader.read_event().ok()? {
            Event::Start(e) => match lower(e.local_name().as_ref()).as_str() {
                "navlabel" => {
                    in_label = true;
                    label.clear();
                }
                "content" => {
                    if let Some(src) = attr(&e, "src") {
                        if let Some(text) = pending.take() {
                            titles.add(resolve(base_dir, &src), &text);
                        }
                    }
                }
                _ => {}
            },
            Event::End(e) => {
                if lower(e.local_name().as_ref()) == "navlabel" {
                    in_label = false;
                    pending = Some(std::mem::take(&mut label));
                }
            }
            Event::Text(t) if in_label => label.push_str(&t.decode().ok()?),
            Event::GeneralRef(r) if in_label => label.push_str(&entity(&r).unwrap_or_default()),
            Event::Eof => break,
            _ => {}
        }
    }
    Some(titles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_nav_toc_gives_titles_by_resolved_path() {
        let nav = r##"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body>
<nav epub:type="landmarks"><ol><li><a href="cover.xhtml">Capa</a></li></ol></nav>
<nav epub:type="toc"><ol>
  <li><a href="text/c1.xhtml">Cap&#237;tulo   Um</a></li>
  <li><a href="text/c2.xhtml#inicio">Dois &amp; Meio</a>
    <ol><li><a href="text/c2.xhtml#sub">Subseção</a></li></ol></li>
</ol></nav>
</body></html>"##;
        let titles = parse_nav(nav.as_bytes(), "OEBPS").unwrap();
        assert_eq!(titles.get("OEBPS/text/c1.xhtml"), Some("Capítulo Um"));
        // The first link to a file names it.
        assert_eq!(titles.get("OEBPS/text/c2.xhtml"), Some("Dois & Meio"));
        assert_eq!(titles.get("OEBPS/cover.xhtml"), None);
    }

    #[test]
    fn the_first_nav_is_used_when_none_is_typed_toc() {
        let nav = r#"<body><nav><ol><li><a href="a.xhtml">A</a></li></ol></nav></body>"#;
        let titles = parse_nav(nav.as_bytes(), "").unwrap();
        assert_eq!(titles.get("a.xhtml"), Some("A"));
        assert!(parse_nav(b"<body/>", "").is_none());
    }

    #[test]
    fn the_ncx_gives_titles() {
        let ncx = r#"<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap>
<navPoint id="n1"><navLabel><text>Primeiro</text></navLabel><content src="c1.xhtml"/>
  <navPoint id="n2"><navLabel><text>Aninhado</text></navLabel><content src="c2.xhtml#x"/></navPoint>
</navPoint>
</navMap></ncx>"#;
        let titles = parse_ncx(ncx.as_bytes(), "OEBPS").unwrap();
        assert_eq!(titles.get("OEBPS/c1.xhtml"), Some("Primeiro"));
        assert_eq!(titles.get("OEBPS/c2.xhtml"), Some("Aninhado"));
    }

    #[test]
    fn titles_fill_only_missing_paths() {
        let mut first = parse_nav(
            br#"<nav epub:type="toc"><a href="a.xhtml">Nav</a></nav>"#,
            "",
        )
        .unwrap();
        let second = parse_ncx(
            br#"<navMap><navPoint><navLabel><text>Ncx A</text></navLabel><content src="a.xhtml"/></navPoint>
<navPoint><navLabel><text>Ncx B</text></navLabel><content src="b.xhtml"/></navPoint></navMap>"#,
            "",
        )
        .unwrap();
        first.fill_from(second);
        assert_eq!(first.get("a.xhtml"), Some("Nav"));
        assert_eq!(first.get("b.xhtml"), Some("Ncx B"));
    }
}
