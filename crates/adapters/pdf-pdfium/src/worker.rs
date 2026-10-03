//! The PDFium worker thread: owns the library binding and every open document.

use std::{
    collections::HashMap,
    io::Cursor,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::mpsc,
};

use image::{DynamicImage, ImageFormat};
use nlmx_domain::document::{
    BoundingBox, DocumentError, DocumentHandle, DocumentMetadata, PageImage, PageInfo,
    RenderOptions, RenderedPage, TextSpan,
};
use pdfium_render::prelude::*;

use crate::layout::{self, Glyph};

pub type Job = Box<dyn FnOnce(&mut Worker) + Send>;

/// What PDFium returns for a hyphen that ends a line (a hyphenation point), instead of "-".
const LINE_END_HYPHEN: char = '\u{2}';

pub struct Worker {
    pdfium: &'static Pdfium,
    documents: HashMap<u64, PdfDocument<'static>>,
    next_handle: u64,
}

impl Worker {
    /// Starts the worker and waits until the library is bound (or failed to bind).
    pub fn spawn(library_path: PathBuf) -> Result<mpsc::Sender<Job>, DocumentError> {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (ready, started) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("pdfium".into())
            .spawn(move || {
                let bindings = match Pdfium::bind_to_library(&library_path) {
                    Ok(bindings) => bindings,
                    Err(err) => {
                        let _ = ready.send(Err(DocumentError::Engine(format!(
                            "não foi possível carregar {}: {err:?}",
                            library_path.display()
                        ))));
                        return;
                    }
                };
                // Documents borrow the library, so it lives for the rest of the process.
                let pdfium: &'static Pdfium = Box::leak(Box::new(Pdfium::new(bindings)));
                let mut worker = Worker {
                    pdfium,
                    documents: HashMap::new(),
                    next_handle: 1,
                };
                let _ = ready.send(Ok(()));
                // Runs until every sender (engine) is dropped; open documents are closed on exit.
                for job in inbox {
                    // A panicking job drops its reply sender (the caller sees an error) but must
                    // not take the worker and every other open document down with it.
                    let _ = catch_unwind(AssertUnwindSafe(|| job(&mut worker)));
                }
            })
            .map_err(|err| {
                DocumentError::Engine(format!("não foi possível iniciar o worker: {err}"))
            })?;

        started
            .recv()
            .map_err(|_| DocumentError::Engine("o worker do PDFium falhou ao iniciar".into()))??;
        Ok(jobs)
    }

    pub fn open(&mut self, path: &Path) -> Result<DocumentHandle, DocumentError> {
        if !path.exists() {
            return Err(DocumentError::NotFound);
        }
        let document = self
            .pdfium
            .load_pdf_from_file(path, None)
            .map_err(map_error)?;
        let handle = self.next_handle;
        self.next_handle += 1;
        self.documents.insert(handle, document);
        Ok(DocumentHandle(handle))
    }

    pub fn close(&mut self, handle: DocumentHandle) -> Result<(), DocumentError> {
        self.documents
            .remove(&handle.0)
            .map(drop)
            .ok_or(DocumentError::UnknownHandle)
    }

    fn document(&self, handle: DocumentHandle) -> Result<&PdfDocument<'static>, DocumentError> {
        self.documents
            .get(&handle.0)
            .ok_or(DocumentError::UnknownHandle)
    }

    fn page(&self, handle: DocumentHandle, number: u32) -> Result<PdfPage<'_>, DocumentError> {
        let document = self.document(handle)?;
        let page_count = document.pages().len() as u32;
        if number == 0 || number > page_count {
            return Err(DocumentError::PageOutOfRange {
                page: number,
                page_count,
            });
        }
        document
            .pages()
            .get((number - 1) as PdfPageIndex)
            .map_err(map_error)
    }

    pub fn page_count(&self, handle: DocumentHandle) -> Result<u32, DocumentError> {
        Ok(self.document(handle)?.pages().len() as u32)
    }

    pub fn metadata(&self, handle: DocumentHandle) -> Result<DocumentMetadata, DocumentError> {
        let document = self.document(handle)?;
        let tag = |kind| {
            document
                .metadata()
                .get(kind)
                .map(|tag| tag.value().trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let date = |kind| tag(kind).map(|raw| layout::pdf_date_to_iso(&raw).unwrap_or(raw));
        // pdfium-render 0.9.4 queries the Info key "ModificationDate" instead of the standard
        // "ModDate", so `modified_at` is only filled for PDFs that (non-standardly) use that key.
        Ok(DocumentMetadata {
            title: tag(PdfDocumentMetadataTagType::Title),
            author: tag(PdfDocumentMetadataTagType::Author),
            subject: tag(PdfDocumentMetadataTagType::Subject),
            keywords: tag(PdfDocumentMetadataTagType::Keywords),
            creator: tag(PdfDocumentMetadataTagType::Creator),
            producer: tag(PdfDocumentMetadataTagType::Producer),
            created_at: date(PdfDocumentMetadataTagType::CreationDate),
            modified_at: date(PdfDocumentMetadataTagType::ModificationDate),
            page_count: document.pages().len() as u32,
            pdf_version: pdf_version(document.version()),
        })
    }

    pub fn page_info(
        &self,
        handle: DocumentHandle,
        number: u32,
    ) -> Result<PageInfo, DocumentError> {
        let page = self.page(handle, number)?;
        let text = all_text(&page)?;
        let char_count = text.chars().filter(|c| !c.is_whitespace()).count() as u32;
        let image_count = page
            .objects()
            .iter()
            .filter(|object| object.object_type() == PdfPageObjectType::Image)
            .count() as u32;
        Ok(PageInfo {
            number,
            width: page.width().value,
            height: page.height().value,
            char_count,
            image_count,
            has_text: char_count > 0,
        })
    }

    pub fn pages_without_text(&self, handle: DocumentHandle) -> Result<Vec<u32>, DocumentError> {
        let mut pages = Vec::new();
        for number in 1..=self.page_count(handle)? {
            if !self.page_info(handle, number)?.has_text {
                pages.push(number);
            }
        }
        Ok(pages)
    }

    pub fn extract_text(
        &self,
        handle: DocumentHandle,
        number: u32,
    ) -> Result<String, DocumentError> {
        let page = self.page(handle, number)?;
        let text = all_text(&page)?;
        Ok(text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace(LINE_END_HYPHEN, "-"))
    }

    pub fn text_spans(
        &self,
        handle: DocumentHandle,
        number: u32,
    ) -> Result<Vec<TextSpan>, DocumentError> {
        let page = self.page(handle, number)?;
        let frame = Frame::of(&page);
        let text = page.text().map_err(map_error)?;
        let chars = text.chars();
        let glyphs = chars.iter().filter_map(|c| {
            let ch = match c.unicode_char()? {
                // PDFium reports a hyphen at the end of a line as U+0002.
                LINE_END_HYPHEN => '-',
                ch => ch,
            };
            let bbox = c.loose_bounds().ok().map(|rect| frame.display(&rect));
            let font_name = c.font_name();
            // Standard (non-embedded) fonts report no weight, so the name is the fallback signal.
            let heavy_weight = matches!(
                c.font_weight(),
                Some(
                    PdfFontWeight::Weight600
                        | PdfFontWeight::Weight700Bold
                        | PdfFontWeight::Weight800
                        | PdfFontWeight::Weight900
                )
            );
            Some(Glyph {
                ch,
                bbox,
                bold: heavy_weight
                    || c.font_is_bold_reenforced()
                    || layout::name_implies_bold(&font_name),
                italic: c.font_is_italic() || layout::name_implies_italic(&font_name),
                font_size: c.scaled_font_size().value,
                font_name,
            })
        });
        Ok(layout::group_spans(glyphs))
    }

    pub fn page_images(
        &self,
        handle: DocumentHandle,
        number: u32,
    ) -> Result<Vec<PageImage>, DocumentError> {
        let page = self.page(handle, number)?;
        let frame = Frame::of(&page);
        let mut images = Vec::new();
        for object in page.objects().iter() {
            let Some(image_object) = object.as_image_object() else {
                continue;
            };
            let bitmap = image_object.get_raw_image().map_err(map_error)?;
            let bbox = object.bounds().map_err(map_error)?.to_rect();
            images.push(PageImage {
                index: images.len() as u32,
                bbox: frame.display(&bbox),
                width_px: bitmap.width(),
                height_px: bitmap.height(),
                png: encode_png(&bitmap)?,
            });
        }
        Ok(images)
    }

    pub fn render_page(
        &self,
        handle: DocumentHandle,
        number: u32,
        options: RenderOptions,
    ) -> Result<RenderedPage, DocumentError> {
        if !(options.scale.is_finite() && options.scale > 0.0) {
            return Err(DocumentError::Engine(format!(
                "escala inválida: {}",
                options.scale
            )));
        }
        let page = self.page(handle, number)?;
        let mut config = PdfRenderConfig::new().scale_page_by_factor(options.scale);
        if let Some(max) = options.max_width_px {
            config = config.set_maximum_width(max as Pixels);
        }
        let image = page
            .render_with_config(&config)
            .map_err(map_error)?
            .as_image()
            .map_err(map_error)?;
        Ok(RenderedPage {
            width_px: image.width(),
            height_px: image.height(),
            png: encode_png(&image)?,
        })
    }
}

/// All the text of a page. `PdfPageText::all()` bounds it by the *rotated* page size, which
/// misses everything on a page with `/Rotate 90/270`; the unrotated frame covers it all.
fn all_text(page: &PdfPage<'_>) -> Result<String, DocumentError> {
    let frame = Frame::of(page);
    let size = frame.width.max(frame.height);
    Ok(page
        .text()
        .map_err(map_error)?
        .inside_rect(PdfRect::new_from_values(0.0, 0.0, size, size)))
}

/// How a page is displayed: PDF rectangles (bottom-left origin, unrotated page space) become
/// boxes in the rendered, rotated page with a top-left origin.
struct Frame {
    /// Unrotated page size.
    width: f32,
    height: f32,
    rotation: u32,
}

impl Frame {
    fn of(page: &PdfPage<'_>) -> Self {
        let rotation = match page.rotation() {
            Ok(PdfPageRenderRotation::Degrees90) => 90,
            Ok(PdfPageRenderRotation::Degrees180) => 180,
            Ok(PdfPageRenderRotation::Degrees270) => 270,
            _ => 0,
        };
        // `width()`/`height()` already account for the rotation.
        let (w, h) = (page.width().value, page.height().value);
        let (width, height) = if rotation % 180 == 0 { (w, h) } else { (h, w) };
        Self {
            width,
            height,
            rotation,
        }
    }

    fn display(&self, rect: &PdfRect) -> BoundingBox {
        layout::to_display_box(
            (
                rect.left().value,
                rect.bottom().value,
                rect.right().value,
                rect.top().value,
            ),
            self.width,
            self.height,
            self.rotation,
        )
    }
}

fn encode_png(image: &DynamicImage) -> Result<Vec<u8>, DocumentError> {
    let mut png = Cursor::new(Vec::new());
    image
        .write_to(&mut png, ImageFormat::Png)
        .map_err(|err| DocumentError::Engine(format!("falha ao gerar PNG: {err}")))?;
    Ok(png.into_inner())
}

fn pdf_version(version: PdfDocumentVersion) -> Option<String> {
    let number = match version {
        PdfDocumentVersion::Unset => return None,
        PdfDocumentVersion::Pdf1_0 => 10,
        PdfDocumentVersion::Pdf1_1 => 11,
        PdfDocumentVersion::Pdf1_2 => 12,
        PdfDocumentVersion::Pdf1_3 => 13,
        PdfDocumentVersion::Pdf1_4 => 14,
        PdfDocumentVersion::Pdf1_5 => 15,
        PdfDocumentVersion::Pdf1_6 => 16,
        PdfDocumentVersion::Pdf1_7 => 17,
        PdfDocumentVersion::Pdf2_0 => 20,
        PdfDocumentVersion::Other(n) => n,
    };
    Some(format!("{}.{}", number / 10, number % 10))
}

fn map_error(err: PdfiumError) -> DocumentError {
    match err {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            DocumentError::PasswordRequired
        }
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FormatError) => {
            DocumentError::InvalidPdf("estrutura do arquivo corrompida ou não reconhecida".into())
        }
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FileError) => {
            DocumentError::InvalidPdf("não foi possível ler o arquivo".into())
        }
        PdfiumError::IoError(err) if err.kind() == std::io::ErrorKind::NotFound => {
            DocumentError::NotFound
        }
        other => DocumentError::Engine(format!("{other:?}")),
    }
}
