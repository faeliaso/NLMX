//! CitationEngine: finds the `[n]` markers in the model's answer, drops the ones that point to
//! no passage and maps the rest to documents, pages and highlight boxes.

use std::ops::Range;

use nlmx_domain::{
    ingestion::{DocumentId, PageBox},
    retrieval::fold,
    vectors::ChunkId,
    viewer::ViewerTarget,
};

use super::context::{NOT_FOUND_ANSWER, Source};

/// A passage cited by the answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Citation {
    /// Number of the source (`[n]`).
    pub n: usize,
    pub document_id: DocumentId,
    pub chunk_id: ChunkId,
    pub title: String,
    pub page_start: u32,
    pub page_end: u32,
    pub label: String,
    pub bboxes: Vec<PageBox>,
    /// Byte ranges of each `[n]` marker in the cleaned answer.
    pub spans: Vec<Range<usize>>,
}

impl Citation {
    /// Opens the passage: its first page, every box highlighted.
    pub fn viewer_target(&self) -> ViewerTarget {
        ViewerTarget {
            document_id: self.document_id,
            page: self.page_start,
            highlights: self.bboxes.clone(),
        }
    }
}

/// `[página 42]` in the answer.
#[derive(Debug, Clone, PartialEq)]
pub struct PageReference {
    pub page: u32,
    pub document_id: DocumentId,
    /// The source covering that page, if any (its boxes on the page are highlighted).
    pub source: Option<usize>,
    pub highlights: Vec<PageBox>,
    /// Byte ranges of each marker in the cleaned answer.
    pub spans: Vec<Range<usize>>,
}

impl PageReference {
    pub fn viewer_target(&self) -> ViewerTarget {
        ViewerTarget {
            document_id: self.document_id,
            page: self.page,
            highlights: self.highlights.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentRef {
    pub document_id: DocumentId,
    pub title: String,
    /// Cited pages, ascending.
    pub pages: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PageRef {
    pub document_id: DocumentId,
    pub page: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CitedAnswer {
    /// The answer with invalid markers removed and groups written as `[1][3]`.
    pub text: String,
    /// In order of first mention.
    pub citations: Vec<Citation>,
    /// Documents cited, in order of first mention.
    pub documents: Vec<DocumentRef>,
    pub pages: Vec<PageRef>,
    /// The model said the passages don't contain the answer.
    pub not_found: bool,
    /// Numbers that matched no source.
    pub invalid: Vec<usize>,
    /// `[página N]` references, in order of first mention.
    pub page_refs: Vec<PageReference>,
    /// Pages referenced that could not be tied to a document (kept as plain text).
    pub unresolved_pages: Vec<u32>,
}

impl CitedAnswer {
    /// At least one valid citation.
    pub fn grounded(&self) -> bool {
        !self.citations.is_empty()
    }
}

/// Larger groups or ranges are not citations (e.g. "[2020–2024]").
const MAX_GROUP: usize = 10;

pub struct CitationEngine;

impl CitationEngine {
    pub fn resolve(answer: &str, sources: &[Source]) -> CitedAnswer {
        let mut text = String::with_capacity(answer.len());
        let mut citations: Vec<Citation> = Vec::new();
        let mut invalid = Vec::new();
        let mut page_refs: Vec<PageReference> = Vec::new();
        let mut unresolved_pages = Vec::new();
        let mut rest = answer;
        while let Some(open) = rest.find('[') {
            let (before, from) = rest.split_at(open);
            let Some(close) = from.find(']') else {
                break;
            };
            if let Some(page) = parse_page_ref(&from[1..close]) {
                text.push_str(before);
                match resolve_page(page, sources) {
                    Some((document_id, source)) => {
                        let start = text.len();
                        text.push_str(&format!("[página {page}]"));
                        let span = start..text.len();
                        match page_refs
                            .iter_mut()
                            .find(|r| r.page == page && r.document_id == document_id)
                        {
                            Some(r) => r.spans.push(span),
                            None => page_refs.push(PageReference {
                                page,
                                document_id,
                                source: source.map(|s| s.n),
                                highlights: source
                                    .map(|s| {
                                        s.bboxes
                                            .iter()
                                            .filter(|b| b.page == page)
                                            .copied()
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                                spans: vec![span],
                            }),
                        }
                    }
                    None => {
                        unresolved_pages.push(page);
                        text.push_str(&format!("página {page}"));
                    }
                }
                rest = &from[close + 1..];
                continue;
            }
            let Some(numbers) = parse_group(&from[1..close]) else {
                // Not a citation: keep the bracket and continue after it.
                text.push_str(before);
                text.push('[');
                rest = &from[1..];
                continue;
            };
            let (valid, bad): (Vec<usize>, Vec<usize>) = numbers
                .into_iter()
                .partition(|n| (1..=sources.len()).contains(n));
            invalid.extend(bad);
            if valid.is_empty() {
                // Drop the marker together with the space before it.
                text.push_str(before.trim_end_matches(' '));
            } else {
                text.push_str(before);
                for n in valid {
                    let start = text.len();
                    text.push_str(&format!("[{n}]"));
                    let span = start..text.len();
                    match citations.iter_mut().find(|c| c.n == n) {
                        Some(c) => c.spans.push(span),
                        None => citations.push(citation(&sources[n - 1], span)),
                    }
                }
            }
            rest = &from[close + 1..];
        }
        text.push_str(rest);
        let text = text.trim().to_string();

        let mut documents: Vec<DocumentRef> = Vec::new();
        let mut pages: Vec<PageRef> = Vec::new();
        for c in &citations {
            let doc = match documents
                .iter_mut()
                .find(|d| d.document_id == c.document_id)
            {
                Some(d) => d,
                None => {
                    documents.push(DocumentRef {
                        document_id: c.document_id,
                        title: c.title.clone(),
                        pages: Vec::new(),
                    });
                    documents.last_mut().expect("just pushed")
                }
            };
            for page in c.page_start..=c.page_end {
                doc.pages.push(page);
                pages.push(PageRef {
                    document_id: c.document_id,
                    page,
                });
            }
        }
        for d in &mut documents {
            d.pages.sort();
            d.pages.dedup();
        }
        pages.sort();
        pages.dedup();
        invalid.sort();
        invalid.dedup();
        unresolved_pages.dedup();
        CitedAnswer {
            not_found: is_not_found(&text),
            text,
            citations,
            documents,
            pages,
            invalid,
            page_refs,
            unresolved_pages,
        }
    }
}

fn citation(source: &Source, span: Range<usize>) -> Citation {
    Citation {
        n: source.n,
        document_id: source.document_id,
        chunk_id: source.chunk_id,
        title: source.title.clone(),
        page_start: source.page_start,
        page_end: source.page_end,
        label: source.label.clone(),
        bboxes: source.bboxes.clone(),
        spans: vec![span],
    }
}

/// "página 42", "pág. 42", "pagina 42", "p. 42", "pp. 42", "page 42" → 42.
fn parse_page_ref(inner: &str) -> Option<u32> {
    let folded = fold(inner.trim());
    let (word, number) = folded.split_once([' ', '.'])?;
    if !["pagina", "pag", "p", "pp", "page", "pg"].contains(&word) {
        return None;
    }
    let number = number.trim_start_matches(['.', ' ']).trim();
    number.parse().ok().filter(|n| *n >= 1)
}

/// The document of a page reference: the source whose pages contain it, else the only
/// document in the context.
fn resolve_page(page: u32, sources: &[Source]) -> Option<(DocumentId, Option<&Source>)> {
    if let Some(s) = sources
        .iter()
        .find(|s| (s.page_start..=s.page_end).contains(&page))
    {
        return Some((s.document_id, Some(s)));
    }
    let first = sources.first()?.document_id;
    sources
        .iter()
        .all(|s| s.document_id == first)
        .then_some((first, None))
}

/// "1", "1, 3", "1;2", "2–4", "2-4" → numbers; anything else is not a citation.
fn parse_group(inner: &str) -> Option<Vec<usize>> {
    let mut numbers = Vec::new();
    for part in inner.split([',', ';']) {
        let part = part.trim();
        let range: Vec<&str> = part.split(['-', '–', '—']).map(str::trim).collect();
        match range.as_slice() {
            [n] => numbers.push(n.parse().ok()?),
            [a, b] => {
                let (a, b): (usize, usize) = (a.parse().ok()?, b.parse().ok()?);
                if b < a || b - a >= MAX_GROUP {
                    return None;
                }
                numbers.extend(a..=b);
            }
            _ => return None,
        }
        if numbers.len() > MAX_GROUP {
            return None;
        }
    }
    let mut seen = Vec::new();
    numbers.retain(|n| {
        let new = !seen.contains(n);
        seen.push(*n);
        new
    });
    Some(numbers)
}

fn is_not_found(text: &str) -> bool {
    let words = |t: &str| -> String {
        t.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(fold)
            .collect::<Vec<_>>()
            .join(" ")
    };
    // "Não encontrei essa informação" (without the "nos documentos" tail, which varies).
    let phrase = words(NOT_FOUND_ANSWER);
    let key = phrase.split(' ').take(4).collect::<Vec<_>>().join(" ");
    words(text).contains(&key)
}
