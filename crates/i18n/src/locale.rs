use std::sync::atomic::{AtomicU8, Ordering};

/// A language the interface is translated into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Locale {
    PtBr,
    En,
    Es,
}

/// Official fallback (never Portuguese): used for unsupported system languages.
pub const FALLBACK: Locale = Locale::En;

impl Locale {
    pub const ALL: [Locale; 3] = [Locale::PtBr, Locale::En, Locale::Es];

    /// BCP 47 tag, also the catalog file name and the `<html lang>` value.
    pub fn tag(self) -> &'static str {
        match self {
            Locale::PtBr => "pt-BR",
            Locale::En => "en",
            Locale::Es => "es",
        }
    }

    /// Name of the language in itself, as shown in the language picker.
    pub fn native_name(self) -> &'static str {
        match self {
            Locale::PtBr => "Português (Brasil)",
            Locale::En => "English",
            Locale::Es => "Español",
        }
    }

    /// Exact tag (`pt-BR`, `en`, `es`) — what the saved preference holds.
    pub fn from_tag(tag: &str) -> Option<Locale> {
        Self::ALL.into_iter().find(|l| l.tag() == tag)
    }

    /// Maps any system locale (`pt_PT`, `en-GB`, `es-419`, `fr-FR.UTF-8`…) to a supported one by
    /// its base language; `None` when the language is not supported.
    pub fn normalize(raw: &str) -> Option<Locale> {
        let base = raw
            .trim()
            .split(['-', '_', '.', '@'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match base.as_str() {
            "pt" => Some(Locale::PtBr),
            "en" => Some(Locale::En),
            "es" => Some(Locale::Es),
            _ => None,
        }
    }

    fn code(self) -> u8 {
        match self {
            Locale::PtBr => 0,
            Locale::En => 1,
            Locale::Es => 2,
        }
    }

    fn from_code(code: u8) -> Locale {
        match code {
            0 => Locale::PtBr,
            2 => Locale::Es,
            _ => Locale::En,
        }
    }
}

/// Saved preference → the system's first preferred language → English. Only the first system
/// language counts: a Mac set to French with Portuguese second is French, so English.
pub fn resolve(saved: Option<Locale>, system: &[String]) -> Locale {
    saved
        .or_else(|| system.first().and_then(|raw| Locale::normalize(raw)))
        .unwrap_or(FALLBACK)
}

static CURRENT: AtomicU8 = AtomicU8::new(1);

/// The active interface locale.
pub fn current() -> Locale {
    Locale::from_code(CURRENT.load(Ordering::Relaxed))
}

pub fn set_current(locale: Locale) {
    CURRENT.store(locale.code(), Ordering::Relaxed);
}
