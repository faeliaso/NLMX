//! `DocumentEngine` backed by PDFium through `pdfium-render`.
//!
//! PDFium is not thread-safe and its documents are not `Send`, so one dedicated worker thread owns
//! the library and every open document; callers talk to it through a channel and await a reply.

mod layout;
mod worker;

use std::{
    path::{Path, PathBuf},
    sync::mpsc,
};

use nlmx_application::ports::{BoxFuture, DocumentEngine};
use nlmx_domain::document::{
    DocumentError, DocumentHandle, DocumentMetadata, PageImage, PageInfo, RenderOptions,
    RenderedPage, TextSpan,
};
use tokio::sync::oneshot;

use worker::{Job, Worker};

/// PDFium build this adapter is compiled against (`pdfium_7881` feature; see scripts/bootstrap.sh).
pub const PDFIUM_BUILD: &str = "chromium/7881";

const LIBRARY_FILE: &str = "libpdfium.dylib";

pub struct PdfiumDocumentEngine {
    jobs: mpsc::Sender<Job>,
}

impl PdfiumDocumentEngine {
    /// Loads the PDFium library at `library_path` on a new worker thread.
    pub fn new(library_path: impl Into<PathBuf>) -> Result<Self, DocumentError> {
        let jobs = Worker::spawn(library_path.into())?;
        Ok(Self { jobs })
    }

    /// Loads PDFium from the first location found by [`library_path`].
    pub fn from_default_location() -> Result<Self, DocumentError> {
        let path = library_path().ok_or_else(|| {
            DocumentError::Engine(format!(
                "{LIBRARY_FILE} não encontrada (defina NLMX_PDFIUM_PATH ou rode `make bootstrap`)"
            ))
        })?;
        Self::new(path)
    }

    async fn call<T, F>(&self, job: F) -> Result<T, DocumentError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Worker) -> Result<T, DocumentError> + Send + 'static,
    {
        let stopped = || DocumentError::Engine("o motor de PDF foi encerrado".into());
        let (reply, response) = oneshot::channel();
        self.jobs
            .send(Box::new(move |worker| {
                let _ = reply.send(job(worker));
            }))
            .map_err(|_| stopped())?;
        // A dropped reply means the job panicked; the worker itself keeps running.
        response.await.map_err(|_| stopped())?
    }
}

/// Where to find `libpdfium.dylib`, in order: `NLMX_PDFIUM_PATH`; `Contents/Frameworks` of the app
/// bundle; `runtime/lib` of the workspace (debug builds only).
pub fn library_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("NLMX_PDFIUM_PATH") {
        return Some(PathBuf::from(path));
    }
    let bundled = std::env::current_exe().ok().and_then(|exe| {
        Some(
            exe.parent()?
                .parent()?
                .join("Frameworks")
                .join(LIBRARY_FILE),
        )
    });
    if let Some(path) = bundled.filter(|path| path.exists()) {
        return Some(path);
    }
    if cfg!(debug_assertions) {
        let dev = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../runtime/lib")
            .join(LIBRARY_FILE);
        if dev.exists() {
            return Some(dev);
        }
    }
    None
}

impl DocumentEngine for PdfiumDocumentEngine {
    fn open<'a>(&'a self, path: &'a Path) -> BoxFuture<'a, Result<DocumentHandle, DocumentError>> {
        let path = path.to_path_buf();
        Box::pin(self.call(move |w| w.open(&path)))
    }

    fn close(&self, document: DocumentHandle) -> BoxFuture<'_, Result<(), DocumentError>> {
        Box::pin(self.call(move |w| w.close(document)))
    }

    fn page_count(&self, document: DocumentHandle) -> BoxFuture<'_, Result<u32, DocumentError>> {
        Box::pin(self.call(move |w| w.page_count(document)))
    }

    fn metadata(
        &self,
        document: DocumentHandle,
    ) -> BoxFuture<'_, Result<DocumentMetadata, DocumentError>> {
        Box::pin(self.call(move |w| w.metadata(document)))
    }

    fn page_info(
        &self,
        document: DocumentHandle,
        page: u32,
    ) -> BoxFuture<'_, Result<PageInfo, DocumentError>> {
        Box::pin(self.call(move |w| w.page_info(document, page)))
    }

    fn pages_without_text(
        &self,
        document: DocumentHandle,
    ) -> BoxFuture<'_, Result<Vec<u32>, DocumentError>> {
        Box::pin(self.call(move |w| w.pages_without_text(document)))
    }

    fn extract_text(
        &self,
        document: DocumentHandle,
        page: u32,
    ) -> BoxFuture<'_, Result<String, DocumentError>> {
        Box::pin(self.call(move |w| w.extract_text(document, page)))
    }

    fn text_spans(
        &self,
        document: DocumentHandle,
        page: u32,
    ) -> BoxFuture<'_, Result<Vec<TextSpan>, DocumentError>> {
        Box::pin(self.call(move |w| w.text_spans(document, page)))
    }

    fn page_images(
        &self,
        document: DocumentHandle,
        page: u32,
    ) -> BoxFuture<'_, Result<Vec<PageImage>, DocumentError>> {
        Box::pin(self.call(move |w| w.page_images(document, page)))
    }

    fn render_page(
        &self,
        document: DocumentHandle,
        page: u32,
        options: RenderOptions,
    ) -> BoxFuture<'_, Result<RenderedPage, DocumentError>> {
        Box::pin(self.call(move |w| w.render_page(document, page, options)))
    }
}
