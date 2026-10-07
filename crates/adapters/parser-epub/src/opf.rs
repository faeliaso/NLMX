//! `META-INF/container.xml` and the package document (OPF): metadata, manifest and spine.

use nlmx_domain::parsed::DocumentMetadata;
use quick_xml::events::Event;

use crate::xml::{attr, collapse, entity, lower, reader};

/// The path of the package document, from `container.xml`.
pub(crate) fn rootfile(container: &[u8]) -> Option<String> {
    let mut reader = reader(container);
    let mut found: Option<(bool, String)> = None;
    loop {
        match reader.read_event().ok()? {
            Event::Start(e) if lower(e.local_name().as_ref()) == "rootfile" => {
                let Some(path) = attr(&e, "full-path").filter(|p| !p.is_empty()) else {
                    continue;
                };
                let is_opf =
                    attr(&e, "media-type").is_none_or(|m| m == "application/oebps-package+xml");
                // The first package document wins.
                if found.is_none() || (!found.as_ref().is_some_and(|f| f.0) && is_opf) {
                    found = Some((is_opf, path));
                }
            }
            Event::Eof => return found.map(|f| f.1),
            _ => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    pub id: String,
    pub href: String,
    pub media_type: String,
    pub properties: Vec<String>,
}

impl Item {
    pub(crate) fn is_nav(&self) -> bool {
        self.properties.iter().any(|p| p == "nav")
    }

    pub(crate) fn is_content(&self) -> bool {
        matches!(
            self.media_type.as_str(),
            "application/xhtml+xml" | "text/html" | "application/x-dtbook+xml"
        ) && !self.is_nav()
    }

    pub(crate) fn is_ncx(&self) -> bool {
        self.media_type == "application/x-dtbncx+xml"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpineRef {
    pub idref: String,
    /// `linear="no"`: content outside the main reading order (it is still read).
    pub linear: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Package {
    pub metadata: DocumentMetadata,
    pub manifest: Vec<Item>,
    pub spine: Vec<SpineRef>,
    /// `id` of the NCX item, from `<spine toc="...">`.
    pub toc_id: Option<String>,
}

impl Package {
    pub(crate) fn item(&self, id: &str) -> Option<&Item> {
        self.manifest.iter().find(|item| item.id == id)
    }

    /// The EPUB 3 navigation document.
    pub(crate) fn nav(&self) -> Option<&Item> {
        self.manifest.iter().find(|item| item.is_nav())
    }

    /// The EPUB 2 NCX.
    pub(crate) fn ncx(&self) -> Option<&Item> {
        self.toc_id
            .as_deref()
            .and_then(|id| self.item(id))
            .or_else(|| self.manifest.iter().find(|item| item.is_ncx()))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Title,
    Creator,
    Language,
    Publisher,
    Date,
    Subject,
    Modified,
}

/// Reads the package document. `None` when the XML cannot be read.
pub(crate) fn parse_opf(bytes: &[u8]) -> Option<Package> {
    let mut reader = reader(bytes);
    let mut package = Package::default();
    let mut stack: Vec<String> = Vec::new();
    // The metadata element being read: its field, its depth and the text so far.
    let mut capture: Option<(Field, usize, String)> = None;
    let mut creators: Vec<String> = Vec::new();
    let mut subjects: Vec<String> = Vec::new();

    loop {
        match reader.read_event().ok()? {
            Event::Start(e) => {
                let name = lower(e.local_name().as_ref());
                let in_metadata = stack.last().is_some_and(|p| p == "metadata");
                let in_manifest = stack.last().is_some_and(|p| p == "manifest");
                let in_spine = stack.last().is_some_and(|p| p == "spine");
                match name.as_str() {
                    "package" => package.metadata.source_version = attr(&e, "version"),
                    "spine" => package.toc_id = attr(&e, "toc"),
                    "item" if in_manifest => {
                        if let (Some(id), Some(href)) = (attr(&e, "id"), attr(&e, "href")) {
                            package.manifest.push(Item {
                                id,
                                href,
                                media_type: attr(&e, "media-type").unwrap_or_default(),
                                properties: attr(&e, "properties")
                                    .unwrap_or_default()
                                    .split_whitespace()
                                    .map(str::to_string)
                                    .collect(),
                            });
                        }
                    }
                    "itemref" if in_spine => {
                        if let Some(idref) = attr(&e, "idref") {
                            package.spine.push(SpineRef {
                                idref,
                                linear: attr(&e, "linear").is_none_or(|l| l != "no"),
                            });
                        }
                    }
                    _ if in_metadata && capture.is_none() => {
                        let field = match name.as_str() {
                            "title" => Some(Field::Title),
                            "creator" => Some(Field::Creator),
                            "language" => Some(Field::Language),
                            "publisher" => Some(Field::Publisher),
                            "date" => Some(Field::Date),
                            "subject" => Some(Field::Subject),
                            "meta"
                                if attr(&e, "property").as_deref() == Some("dcterms:modified") =>
                            {
                                Some(Field::Modified)
                            }
                            _ => None,
                        };
                        if let Some(field) = field {
                            capture = Some((field, stack.len() + 1, String::new()));
                        }
                    }
                    _ => {}
                }
                stack.push(name);
            }
            Event::End(_) => {
                if let Some((field, _, text)) = capture.take_if(|c| c.1 == stack.len()) {
                    let text = collapse(&text);
                    if !text.is_empty() {
                        let metadata = &mut package.metadata;
                        match field {
                            Field::Title => {
                                metadata.title.get_or_insert(text);
                            }
                            Field::Language => {
                                metadata.language.get_or_insert(text);
                            }
                            Field::Publisher => {
                                metadata.publisher.get_or_insert(text);
                            }
                            Field::Date => {
                                metadata.created_at.get_or_insert(text);
                            }
                            Field::Modified => {
                                metadata.modified_at.get_or_insert(text);
                            }
                            Field::Creator => creators.push(text),
                            Field::Subject => subjects.push(text),
                        }
                    }
                }
                stack.pop();
            }
            Event::Text(t) => {
                if let Some((_, _, buf)) = capture.as_mut() {
                    buf.push_str(&t.decode().ok()?);
                }
            }
            Event::CData(t) => {
                if let Some((_, _, buf)) = capture.as_mut() {
                    buf.push_str(&t.decode().ok()?);
                }
            }
            Event::GeneralRef(r) => {
                if let Some((_, _, buf)) = capture.as_mut() {
                    buf.push_str(&entity(&r).unwrap_or_default());
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !creators.is_empty() {
        package.metadata.author = Some(creators.join(", "));
    }
    if !subjects.is_empty() {
        package.metadata.subject = Some(subjects.join(", "));
    }
    Some(package)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="id">urn:uuid:1234</dc:identifier>
    <dc:title>A Viagem &amp; o Retorno</dc:title>
    <dc:creator>Ana Souza</dc:creator>
    <dc:creator>Bruno Lima</dc:creator>
    <dc:language>pt-BR</dc:language>
    <dc:publisher>Editora Exemplo</dc:publisher>
    <dc:date>2023-05-01</dc:date>
    <dc:subject>Viagens</dc:subject>
    <dc:subject>Ficção</dc:subject>
    <meta property="dcterms:modified">2024-02-03T10:00:00Z</meta>
    <meta name="cover" content="img"/>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="c1" href="c%C3%A31.xhtml" media-type="application/xhtml+xml"/>
    <item id="img" href="a.png" media-type="image/png"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="c1"/>
    <itemref idref="img" linear="no"/>
  </spine>
</package>"#;

    #[test]
    fn metadata_manifest_and_spine_are_read() {
        let package = parse_opf(OPF.as_bytes()).unwrap();
        let m = &package.metadata;
        assert_eq!(m.title.as_deref(), Some("A Viagem & o Retorno"));
        assert_eq!(m.author.as_deref(), Some("Ana Souza, Bruno Lima"));
        assert_eq!(m.language.as_deref(), Some("pt-BR"));
        assert_eq!(m.publisher.as_deref(), Some("Editora Exemplo"));
        assert_eq!(m.created_at.as_deref(), Some("2023-05-01"));
        assert_eq!(m.modified_at.as_deref(), Some("2024-02-03T10:00:00Z"));
        assert_eq!(m.subject.as_deref(), Some("Viagens, Ficção"));
        assert_eq!(m.source_version.as_deref(), Some("3.0"));

        assert_eq!(package.manifest.len(), 4);
        assert_eq!(package.nav().map(|i| i.id.as_str()), Some("nav"));
        assert_eq!(package.ncx().map(|i| i.id.as_str()), Some("ncx"));
        assert!(package.item("c1").unwrap().is_content());
        assert!(!package.item("nav").unwrap().is_content());
        assert!(!package.item("img").unwrap().is_content());
        let spine: Vec<_> = package
            .spine
            .iter()
            .map(|s| (s.idref.as_str(), s.linear))
            .collect();
        assert_eq!(spine, [("c1", true), ("img", false)]);
    }

    #[test]
    fn the_rootfile_is_found_in_the_container() {
        let container = r#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;
        assert_eq!(
            rootfile(container.as_bytes()).as_deref(),
            Some("OEBPS/content.opf")
        );
        assert_eq!(rootfile(b"<container/>"), None);
        assert_eq!(rootfile(b"<rootfile media-type='x'/>"), None);
    }

    #[test]
    fn a_missing_title_stays_empty() {
        let package = parse_opf(b"<package><metadata></metadata></package>").unwrap();
        assert_eq!(package.metadata.title, None);
        assert!(package.spine.is_empty());
    }
}
