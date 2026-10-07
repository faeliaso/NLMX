//! The PDF viewer's data: document outline (page sizes, without opening the PDF), page images,
//! the text layer (for selection) and in-document search. Text spans are cached per page so
//! that search and the text layer don't reopen the PDF for every page.

use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::{Arc, Mutex},
};

use nlmx_domain::{
    document::{BoundingBox, RenderOptions, TextSpan},
    ingestion::DocumentId,
    viewer::find_in_spans,
};

use crate::ports::{DocumentEngine, DocumentRepository};

#[derive(Debug, Clone, PartialEq)]
pub struct PageSize {
    pub number: u32,
    /// In PDF points.
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DocumentOutline {
    pub document_id: DocumentId,
    /// Identifies the file (prefix of its SHA-256). Page image URLs carry it, so a cached image
    /// is never shown for another document that got the same id after a removal.
    pub version: String,
    pub title: String,
    pub pages: Vec<PageSize>,
}

impl DocumentOutline {
    pub fn page_count(&self) -> u32 {
        self.pages.len() as u32
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PageImage {
    pub png: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub page: u32,
    pub boxes: Vec<BoundingBox>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    /// More matches exist than `MAX_SEARCH_HITS`.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageError {
    NotFound,
    Failed(String),
}

impl std::fmt::Display for PageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("página ou documento não encontrado"),
            Self::Failed(m) => write!(f, "não foi possível abrir o documento: {m}"),
        }
    }
}

/// Largest rendered width accepted (keeps memory bounded).
pub const MAX_WIDTH_PX: u32 = 2400;
pub const MAX_SEARCH_HITS: usize = 500;
/// Pages whose text spans are kept in memory.
const SPAN_CACHE_PAGES: usize = 600;

type SpanKey = (DocumentId, u32);

#[derive(Default)]
struct SpanCache {
    pages: HashMap<SpanKey, Arc<Vec<TextSpan>>>,
    order: VecDeque<SpanKey>,
}

impl SpanCache {
    fn get(&self, key: SpanKey) -> Option<Arc<Vec<TextSpan>>> {
        self.pages.get(&key).cloned()
    }

    fn put(&mut self, key: SpanKey, spans: Arc<Vec<TextSpan>>) {
        if self.pages.insert(key, spans).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > SPAN_CACHE_PAGES {
            if let Some(old) = self.order.pop_front() {
                self.pages.remove(&old);
            }
        }
    }
}

pub struct ViewDocument {
    engine: Arc<dyn DocumentEngine>,
    documents: Arc<dyn DocumentRepository>,
    spans: Mutex<SpanCache>,
}

fn failed(e: impl std::fmt::Display) -> PageError {
    PageError::Failed(e.to_string())
}

impl ViewDocument {
    pub fn new(engine: Arc<dyn DocumentEngine>, documents: Arc<dyn DocumentRepository>) -> Self {
        Self {
            engine,
            documents,
            spans: Mutex::new(SpanCache::default()),
        }
    }

    /// Drops the cached text of a removed document (its id may be given to the next import).
    pub fn forget(&self, document: DocumentId) {
        let mut cache = self.spans.lock().unwrap();
        cache.pages.retain(|(doc, _), _| *doc != document);
        cache.order.retain(|(doc, _)| *doc != document);
    }

    /// Title and page sizes, from the database.
    pub async fn outline(&self, document: DocumentId) -> Result<DocumentOutline, PageError> {
        let summary = self
            .documents
            .list()
            .await
            .map_err(|e| failed(e.message))?
            .into_iter()
            .find(|d| d.id == document)
            .ok_or(PageError::NotFound)?;
        let pages = self
            .documents
            .pages(document)
            .await
            .map_err(|e| failed(e.message))?;
        if pages.is_empty() {
            return Err(PageError::Failed(
                "o documento ainda não foi processado".into(),
            ));
        }
        let record = self
            .documents
            .get(document)
            .await
            .map_err(|e| failed(e.message))?
            .ok_or(PageError::NotFound)?;
        Ok(DocumentOutline {
            document_id: document,
            version: record.sha256.chars().take(12).collect(),
            title: summary.title,
            pages: pages
                .into_iter()
                .map(|p| PageSize {
                    number: p.number,
                    width: p.width,
                    height: p.height,
                })
                .collect(),
        })
    }

    /// Runs `f` on the opened document, closing it afterwards.
    async fn with_document<T>(
        &self,
        document: DocumentId,
        f: impl AsyncFnOnce(nlmx_domain::document::DocumentHandle) -> Result<T, PageError>,
    ) -> Result<T, PageError> {
        let record = self
            .documents
            .get(document)
            .await
            .map_err(|e| failed(e.message))?
            .ok_or(PageError::NotFound)?;
        let handle = self
            .engine
            .open(Path::new(&record.library_path))
            .await
            .map_err(failed)?;
        let result = f(handle).await;
        let _ = self.engine.close(handle).await;
        result
    }

    pub async fn render(
        &self,
        document: DocumentId,
        page: u32,
        width_px: u32,
    ) -> Result<PageImage, PageError> {
        let width = width_px.clamp(80, MAX_WIDTH_PX);
        self.with_document(document, async |handle| {
            let count = self.engine.page_count(handle).await.map_err(failed)?;
            if page == 0 || page > count {
                return Err(PageError::NotFound);
            }
            let info = self.engine.page_info(handle, page).await.map_err(failed)?;
            let rendered = self
                .engine
                .render_page(
                    handle,
                    page,
                    RenderOptions {
                        scale: width as f32 / info.width.max(1.0),
                        max_width_px: Some(width),
                    },
                )
                .await
                .map_err(failed)?;
            Ok(PageImage { png: rendered.png })
        })
        .await
    }

    /// Text spans of `pages` (all cached afterwards), opening the PDF only for missing ones.
    async fn spans(
        &self,
        document: DocumentId,
        pages: &[u32],
    ) -> Result<Vec<(u32, Arc<Vec<TextSpan>>)>, PageError> {
        let missing: Vec<u32> = {
            let cache = self.spans.lock().unwrap();
            pages
                .iter()
                .copied()
                .filter(|p| cache.get((document, *p)).is_none())
                .collect()
        };
        if !missing.is_empty() {
            self.with_document(document, async |handle| {
                let count = self.engine.page_count(handle).await.map_err(failed)?;
                for page in missing {
                    if page == 0 || page > count {
                        return Err(PageError::NotFound);
                    }
                    let spans = self.engine.text_spans(handle, page).await.map_err(failed)?;
                    self.spans
                        .lock()
                        .unwrap()
                        .put((document, page), Arc::new(spans));
                }
                Ok(())
            })
            .await?;
        }
        let cache = self.spans.lock().unwrap();
        Ok(pages
            .iter()
            .filter_map(|p| cache.get((document, *p)).map(|s| (*p, s)))
            .collect())
    }

    /// Text with positions, for the selectable text layer.
    pub async fn text_layer(
        &self,
        document: DocumentId,
        page: u32,
    ) -> Result<Arc<Vec<TextSpan>>, PageError> {
        self.spans(document, &[page])
            .await?
            .pop()
            .map(|(_, s)| s)
            .ok_or(PageError::NotFound)
    }

    /// Every occurrence of `query` in the document, in page order.
    pub async fn search(
        &self,
        document: DocumentId,
        query: &str,
    ) -> Result<SearchResults, PageError> {
        if query.trim().is_empty() {
            return Ok(SearchResults {
                hits: Vec::new(),
                truncated: false,
            });
        }
        let outline = self.outline(document).await?;
        let pages: Vec<u32> = outline.pages.iter().map(|p| p.number).collect();
        let mut hits = Vec::new();
        for (page, spans) in self.spans(document, &pages).await? {
            for boxes in find_in_spans(&spans, query) {
                if hits.len() == MAX_SEARCH_HITS {
                    return Ok(SearchResults {
                        hits,
                        truncated: true,
                    });
                }
                hits.push(SearchHit { page, boxes });
            }
        }
        Ok(SearchResults {
            hits,
            truncated: false,
        })
    }
}
