//! A whole book: container → package → spine → chapters → sections.

use nlmx_domain::{
    document_type::DocumentType,
    parsed::{
        ContentBlock, ContentKind, DocumentSection, ParseError, ParseWarning, ParsedDocument,
    },
    source::SourceLocation,
};

use crate::{
    archive::{Container, Limits, ReadFailure, dir_of, resolve},
    opf::{self, Item, Package},
    toc::{self, Titles},
    xhtml::{self, RawBlock},
    xml::{attr, collapse, lower, reader},
};

const CONTAINER: &str = "META-INF/container.xml";
const ENCRYPTION: &str = "META-INF/encryption.xml";

/// Encryption that only obfuscates embedded fonts; the text is readable.
const FONT_OBFUSCATION: [&str; 2] = [
    "http://www.idpf.org/2008/embedding",
    "http://ns.adobe.com/pdf/enc#RC",
];

const INVALID: ParseError = ParseError::Invalid(DocumentType::Epub);

/// Parses an EPUB held in memory, with the default limits.
pub fn parse_bytes(bytes: &[u8]) -> Result<ParsedDocument, ParseError> {
    parse_bytes_with_limits(bytes, Limits::default())
}

pub fn parse_bytes_with_limits(bytes: &[u8], limits: Limits) -> Result<ParsedDocument, ParseError> {
    let mut container = Container::open(bytes, limits)?;
    check_drm(&mut container)?;

    let container_xml = read_required(&mut container, CONTAINER)?;
    let opf_path = opf::rootfile(&container_xml).ok_or(INVALID)?;
    let opf_path = resolve("", &opf_path);
    let opf_dir = dir_of(&opf_path).to_string();
    let package = opf::parse_opf(&read_required(&mut container, &opf_path)?).ok_or(INVALID)?;

    let chapters = chapters(&package);
    if chapters.is_empty() {
        return Err(INVALID);
    }
    let titles = titles(&mut container, &package, &opf_dir)?;

    let mut sections = Vec::new();
    let mut warnings = Vec::new();
    let mut skipped = 0usize;
    for (position, item) in chapters.iter().enumerate() {
        let index = position as u32 + 1;
        let path = resolve(&opf_dir, &item.href);
        let blocks = match container.read(&path) {
            Ok(Some(bytes)) => xhtml::parse_chapter(&bytes).ok(),
            Ok(None) | Err(ReadFailure::Corrupt) => None,
            Err(ReadFailure::TooLarge) => return Err(ParseError::TooLarge),
        };
        let Some(blocks) = blocks else {
            skipped += 1;
            warnings.push(ParseWarning::SkippedUnit { index });
            continue;
        };
        if blocks.is_empty() {
            continue;
        }
        sections.extend(chapter_sections(index, titles.get(&path), blocks));
    }

    if skipped == chapters.len() {
        return Err(INVALID);
    }
    if sections.is_empty() {
        return Err(ParseError::Empty);
    }
    ParsedDocument::new(
        DocumentType::Epub,
        package.metadata,
        Vec::new(),
        sections,
        warnings,
    )
    .map_err(|error| ParseError::Engine(error.to_string()))
}

/// An entry the book cannot do without.
fn read_required(container: &mut Container<'_>, name: &str) -> Result<Vec<u8>, ParseError> {
    match container.read(name) {
        Ok(Some(bytes)) => Ok(bytes),
        Ok(None) | Err(ReadFailure::Corrupt) => Err(INVALID),
        Err(ReadFailure::TooLarge) => Err(ParseError::TooLarge),
    }
}

/// `Drm` when `encryption.xml` lists anything but font obfuscation.
fn check_drm(container: &mut Container<'_>) -> Result<(), ParseError> {
    let bytes = match container.read(ENCRYPTION) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Ok(()),
        Err(ReadFailure::TooLarge) => return Err(ParseError::TooLarge),
        Err(ReadFailure::Corrupt) => return Err(INVALID),
    };
    let mut reader = reader(&bytes);
    loop {
        match reader.read_event().map_err(|_| INVALID)? {
            quick_xml::events::Event::Start(e)
                if lower(e.local_name().as_ref()) == "encryptionmethod" =>
            {
                let algorithm = attr(&e, "algorithm").unwrap_or_default();
                if !FONT_OBFUSCATION.contains(&algorithm.as_str()) {
                    return Err(ParseError::Drm);
                }
            }
            quick_xml::events::Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

/// The content documents in reading order. Items marked `linear="no"` (notes, alternate
/// covers) are kept in their place: they are part of the book and dropping them would lose
/// text. The navigation document and non-text resources are not chapters.
fn chapters(package: &Package) -> Vec<&Item> {
    package
        .spine
        .iter()
        .filter_map(|spine| package.item(&spine.idref))
        .filter(|item| item.is_content())
        .collect()
}

/// Chapter titles from the EPUB 3 navigation document, completed by the NCX. A table of
/// contents that cannot be read only costs the titles.
fn titles(
    container: &mut Container<'_>,
    package: &Package,
    opf_dir: &str,
) -> Result<Titles, ParseError> {
    let mut titles = Titles::default();
    let mut read = |item: Option<&Item>,
                    parse: fn(&[u8], &str) -> Option<Titles>|
     -> Result<Option<Titles>, ParseError> {
        let Some(item) = item else {
            return Ok(None);
        };
        let path = resolve(opf_dir, &item.href);
        match container.read(&path) {
            Ok(Some(bytes)) => Ok(parse(&bytes, dir_of(&path))),
            Ok(None) | Err(ReadFailure::Corrupt) => Ok(None),
            Err(ReadFailure::TooLarge) => Err(ParseError::TooLarge),
        }
    };
    if let Some(nav) = read(package.nav(), toc::parse_nav)? {
        titles.fill_from(nav);
    }
    if let Some(ncx) = read(package.ncx(), toc::parse_ncx)? {
        titles.fill_from(ncx);
    }
    Ok(titles)
}

/// The sections of one chapter: the chapter's title is the root (level 1) and the headings
/// inside it nest relative to it. Every section starts with its heading block.
fn chapter_sections(
    index: u32,
    toc_title: Option<&str>,
    raw: Vec<RawBlock>,
) -> Vec<DocumentSection> {
    let first_heading = raw.iter().find_map(|b| match b.kind {
        ContentKind::Heading { .. } => Some(b.text.as_str()),
        _ => None,
    });
    let title = toc_title
        .map(collapse)
        .filter(|t| !t.is_empty())
        .or_else(|| first_heading.map(str::to_string))
        .unwrap_or_else(|| format!("Capítulo {index}"));

    let location = |section: Option<&str>| SourceLocation::Epub {
        chapter_index: index,
        chapter_title: Some(title.clone()),
        section: section.map(str::to_string),
    };
    let heading = |level: u8, text: &str, section: Option<&str>| ContentBlock {
        kind: ContentKind::Heading { level },
        text: text.to_string(),
        location: location(section),
    };

    // The chapter's own heading may already be the first block.
    let mut blocks = raw.into_iter().peekable();
    let root_raw_level = match blocks.peek() {
        Some(RawBlock {
            kind: ContentKind::Heading { level },
            text,
        }) if *text == title => {
            let level = *level;
            blocks.next();
            level
        }
        _ => 0,
    };

    struct Open {
        title: Option<String>,
        level: u8,
        path: Vec<String>,
        blocks: Vec<ContentBlock>,
        /// Name of the section for the locations of its blocks.
        name: Option<String>,
    }
    let mut open = Open {
        title: Some(title.clone()),
        level: 1,
        path: vec![title.clone()],
        blocks: vec![heading(1, &title, None)],
        name: None,
    };
    let mut done = Vec::new();
    // Enclosing headings as (level in the source, text); the chapter title is never popped.
    let mut stack: Vec<(u8, String)> = vec![(root_raw_level, title.clone())];

    for block in blocks {
        match block.kind {
            ContentKind::Heading { level: raw_level } => {
                while stack.len() > 1 && stack.last().is_some_and(|(l, _)| *l >= raw_level) {
                    stack.pop();
                }
                let level = (stack.len() + 1).min(u8::MAX as usize) as u8;
                let mut path: Vec<String> = stack.iter().map(|(_, t)| t.clone()).collect();
                path.push(block.text.clone());
                stack.push((raw_level, block.text.clone()));
                let previous = std::mem::replace(
                    &mut open,
                    Open {
                        title: Some(block.text.clone()),
                        level,
                        path,
                        blocks: vec![heading(level, &block.text, Some(&block.text))],
                        name: Some(block.text),
                    },
                );
                done.push(previous);
            }
            kind => open.blocks.push(ContentBlock {
                kind,
                text: block.text,
                location: location(open.name.as_deref()),
            }),
        }
    }
    done.push(open);
    done.into_iter()
        .filter_map(|s| DocumentSection::new(s.title, s.level, s.path, s.blocks))
        .collect()
}
