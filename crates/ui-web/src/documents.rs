//! View models for the document library.

use nlmx_domain::ingestion::{DocumentStatus, DocumentSummary, RemovalImpact};

use nlmx_i18n::{t_args, t_count};

use crate::formats::format_view;

pub struct DocumentRow {
    pub id: i64,
    /// Not while it is being read (the ingestion would race with the removal).
    pub removable: bool,
    /// What removing it deletes, for the confirmation dialog.
    pub removal: String,
    /// Has extracted pages, so the viewer can open it.
    pub viewable: bool,
    /// Every source has information to show; only a PDF also has a preview (`viewable`).
    pub has_details: bool,
    /// A note has no file: its title is its name and it has no file name or size to show.
    pub is_note: bool,
    pub icon: &'static str,
    pub format: String,
    pub title: String,
    /// Accessible name of the row's menu button.
    pub more_actions: String,
    /// Title of the removal dialog.
    pub remove_title: String,
    pub filename: String,
    pub pages: String,
    pub chunks: String,
    pub status_label: String,
    pub status_kind: &'static str,
    pub error: String,
    pub imported_on: String,
}

impl From<DocumentSummary> for DocumentRow {
    fn from(doc: DocumentSummary) -> Self {
        let (status_label, status_kind) = status_badge(doc.status);
        Self {
            id: doc.id,
            removable: !doc.status.is_unfinished(),
            removal: removal_description(&RemovalImpact {
                chunks: doc.chunk_count,
                ..RemovalImpact::default()
            }),
            viewable: doc.page_count.is_some_and(|n| n > 0)
                && !matches!(doc.status, DocumentStatus::Failed),
            has_details: !doc.document_type.previewable(),
            is_note: doc.document_type.is_note(),
            icon: format_view(doc.document_type).icon,
            format: format_view(doc.document_type).label.to_string(),
            // Pages exist only in a paged format.
            pages: doc
                .page_count
                .map_or_else(String::new, |n| t_count("documents-pages", i64::from(n))),
            more_actions: t_args(
                "documents-more-actions",
                &[("title", doc.title.clone().into())],
            ),
            remove_title: t_args(
                "documents-remove-dialog-title",
                &[("title", doc.title.clone().into())],
            ),
            title: doc.title,
            filename: doc.original_filename,
            chunks: t_count("documents-chunks", i64::from(doc.chunk_count)),
            status_label,
            status_kind,
            error: doc.error.unwrap_or_default(),
            imported_on: date_label(&doc.imported_at),
        }
    }
}

/// "2026-10-02T14:31:05.123Z" → "02/10/2026" (`10/02/2026` in English); the text itself when it
/// is not a timestamp.
pub fn date_label(timestamp: &str) -> String {
    date_label_in(nlmx_i18n::current(), timestamp)
}

pub fn date_label_in(locale: nlmx_i18n::Locale, timestamp: &str) -> String {
    match (
        timestamp.get(0..4),
        timestamp.get(5..7),
        timestamp.get(8..10),
    ) {
        (Some(year), Some(month), Some(day)) => nlmx_i18n::tr_args(
            locale,
            "documents-date",
            &[
                ("year", year.into()),
                ("month", month.into()),
                ("day", day.into()),
            ],
        ),
        _ => timestamp.to_string(),
    }
}

/// Localized label and badge kind of a document status.
pub fn status_badge(status: DocumentStatus) -> (String, &'static str) {
    let (id, kind) = match status {
        DocumentStatus::Embedding => ("documents-status-embedding", "info"),
        DocumentStatus::Indexed => ("documents-status-indexed", "success"),
        DocumentStatus::NeedsOcr => ("documents-status-needs-ocr", "warning"),
        DocumentStatus::Failed => ("documents-status-failed", "danger"),
        DocumentStatus::Queued => ("documents-status-queued", "neutral"),
        DocumentStatus::Extracting => ("documents-status-extracting", "accent"),
        DocumentStatus::Structuring => ("documents-status-structuring", "accent"),
        DocumentStatus::Chunking => ("documents-status-chunking", "accent"),
    };
    (nlmx_i18n::tr(nlmx_i18n::current(), id), kind)
}

/// The document list, or why it cannot be shown.
pub enum Library {
    Unavailable(String),
    Documents(Vec<DocumentRow>),
}

/// The confirmation text: what goes, including the chat history, and that the original stays.
pub fn removal_description(impact: &RemovalImpact) -> String {
    removal_description_in(nlmx_i18n::current(), impact)
}

pub fn removal_description_in(locale: nlmx_i18n::Locale, impact: &RemovalImpact) -> String {
    use nlmx_i18n::{Arg, tr_args};
    let count = |id: &str, n: u32| tr_args(locale, id, &[("count", Arg::from(i64::from(n)))]);
    let mut text = tr_args(
        locale,
        "documents-removal-base",
        &[("chunks", Arg::from(i64::from(impact.chunks)))],
    );
    let conversations = (impact.conversations > 0)
        .then(|| count("documents-removal-conversations", impact.conversations));
    let turns = (impact.turns > 0).then(|| count("documents-removal-turns", impact.turns));
    let history = match (conversations, turns) {
        (Some(first), Some(second)) => Some(tr_args(
            locale,
            "documents-removal-join",
            &[("first", first.into()), ("second", second.into())],
        )),
        (one, other) => one.or(other),
    };
    if let Some(items) = history {
        text.push(' ');
        text.push_str(&tr_args(
            locale,
            "documents-removal-history",
            &[("items", items.into())],
        ));
    }
    text.push(' ');
    text.push_str(&nlmx_i18n::tr(locale, "documents-removal-original"));
    text
}

/// The outcome of an action, announced as a toast once the page is shown.
pub struct Notice {
    pub kind: &'static str,
    /// Shown as a toast (the page carries it in a marker that `app.js` turns into one).
    pub message: String,
}

#[cfg(test)]
mod tests {
    use nlmx_i18n::Locale;

    use super::*;

    #[test]
    fn dates_follow_the_language() {
        let ts = "2026-10-02T14:31:05.123Z";
        assert_eq!(date_label_in(Locale::PtBr, ts), "02/10/2026");
        assert_eq!(date_label_in(Locale::Es, ts), "02/10/2026");
        assert_eq!(date_label_in(Locale::En, ts), "10/02/2026");
        assert_eq!(date_label_in(Locale::En, "soon"), "soon");
    }

    #[test]
    fn removal_text_keeps_the_portuguese_wording() {
        let impact = RemovalImpact {
            chunks: 12,
            conversations: 1,
            turns: 3,
        };
        assert_eq!(
            removal_description_in(Locale::PtBr, &impact),
            "A cópia do documento na biblioteca, seus 12 trechos e os índices de busca serão \
             apagados deste Mac. No Chat, 1 conversa será excluída e 3 pares de pergunta e \
             resposta que o usaram serão removidos. O arquivo original não é afetado."
        );
        let plain = RemovalImpact::default();
        assert_eq!(
            removal_description_in(Locale::PtBr, &plain),
            "A cópia do documento na biblioteca será apagada deste Mac. O arquivo original não é afetado."
        );
        let en = removal_description_in(Locale::En, &impact);
        assert!(
            en.contains("12 passages")
                && en.contains("In Chat, 1 conversation will be deleted and 3")
        );
    }
}
