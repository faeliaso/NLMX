//! PDF document concepts produced by a document engine. Coordinates are PDF points (1/72 inch)
//! with the origin at the **top-left** corner of the page, so they map directly onto rendered pages.

use std::fmt;

/// Opaque reference to a document opened by a document engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DocumentHandle(pub u64);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    /// ISO-8601 when the PDF date could be parsed, otherwise the raw PDF date string.
    pub created_at: Option<String>,
    pub modified_at: Option<String>,
    pub page_count: u32,
    /// e.g. "1.7".
    pub pdf_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BoundingBox {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl BoundingBox {
    pub fn width(&self) -> f32 {
        self.right - self.left
    }

    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }

    /// Smallest box containing both.
    pub fn union(&self, other: &BoundingBox) -> BoundingBox {
        BoundingBox {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PageInfo {
    /// 1-based page number.
    pub number: u32,
    pub width: f32,
    pub height: f32,
    /// Non-whitespace characters in the text layer.
    pub char_count: u32,
    pub image_count: u32,
    /// False for scanned/image-only or blank pages (OCR candidates when `image_count > 0`).
    pub has_text: bool,
}

/// A run of characters on one line sharing the same font and size.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpan {
    pub text: String,
    pub bbox: BoundingBox,
    pub font_name: String,
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PageImage {
    /// Position of the image among the page's images (0-based).
    pub index: u32,
    pub bbox: BoundingBox,
    pub width_px: u32,
    pub height_px: u32,
    pub png: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderOptions {
    /// 1.0 renders at 72 dpi (one pixel per PDF point).
    pub scale: f32,
    /// Caps the output width; the scale is reduced to fit.
    pub max_width_px: Option<u32>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            scale: 2.0,
            max_width_px: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPage {
    pub png: Vec<u8>,
    pub width_px: u32,
    pub height_px: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentError {
    NotFound,
    InvalidPdf(String),
    PasswordRequired,
    PageOutOfRange { page: u32, page_count: u32 },
    UnknownHandle,
    Engine(String),
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("arquivo não encontrado"),
            Self::InvalidPdf(reason) => write!(f, "o arquivo não é um PDF válido: {reason}"),
            Self::PasswordRequired => f.write_str("o PDF é protegido por senha"),
            Self::PageOutOfRange { page, page_count } => {
                write!(
                    f,
                    "a página {page} não existe (o documento tem {page_count})"
                )
            }
            Self::UnknownHandle => f.write_str("documento não está aberto"),
            Self::Engine(reason) => write!(f, "falha no motor de PDF: {reason}"),
        }
    }
}

impl std::error::Error for DocumentError {}
