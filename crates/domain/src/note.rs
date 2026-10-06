//! A note: text pasted by the user, kept in the database instead of a file. Everything else
//! (parsing, normalization, chunks, embeddings, RAG) is the regular document pipeline.

use std::fmt;

/// The longest note accepted, in characters.
pub const MAX_NOTE_CHARS: usize = 1_000_000;

/// A first line longer than this is not a title.
const MAX_TITLE_CHARS: usize = 80;
/// A title derived from the start of the content is cut at this many characters.
const DERIVED_TITLE_CHARS: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteError {
    /// Nothing but whitespace.
    Empty,
    TooLong,
}

impl fmt::Display for NoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("A nota está vazia."),
            Self::TooLong => write!(
                f,
                "A nota é grande demais (no máximo {MAX_NOTE_CHARS} caracteres)."
            ),
        }
    }
}

impl std::error::Error for NoteError {}

/// The text of a note as it is stored: `\n` line ends, no trailing spaces on a line, no blank
/// lines at the ends and at most one blank line in a row. Words, punctuation, indentation and
/// the order of the content are untouched. `Empty` when nothing but whitespace is left.
pub fn clean_note_text(raw: &str) -> Result<String, NoteError> {
    let unified = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(unified.len());
    let mut blank_run = 0;
    for line in unified.split('\n') {
        let line = line.trim_end();
        if line.is_empty() {
            blank_run += 1;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
            if blank_run > 0 {
                out.push('\n');
            }
        }
        blank_run = 0;
        out.push_str(line);
    }
    let out = out.trim_start_matches(['\u{feff}']).to_string();
    if out.trim().is_empty() {
        return Err(NoteError::Empty);
    }
    if out.chars().count() > MAX_NOTE_CHARS {
        return Err(NoteError::TooLong);
    }
    Ok(out)
}

/// The title of a note: its first line when it looks like a title (short and not a sentence),
/// otherwise the start of the content.
pub fn note_title(text: &str) -> String {
    let first = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let heading = first.trim_start_matches('#').trim();
    let looks_like_title = !heading.is_empty()
        && heading.chars().count() <= MAX_TITLE_CHARS
        && !heading.ends_with(['.', '!', '?', ';', ',']);
    if looks_like_title {
        return heading.to_string();
    }
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= DERIVED_TITLE_CHARS {
        return flat;
    }
    let cut: String = flat.chars().take(DERIVED_TITLE_CHARS).collect();
    let cut = match cut.rfind(' ') {
        Some(at) if at > DERIVED_TITLE_CHARS / 2 => cut[..at].to_string(),
        _ => cut,
    };
    format!("{}…", cut.trim_end_matches([',', ';', ':', '.', ' ']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_whitespace_notes_are_refused() {
        for raw in ["", "   ", "\n\n\t \r\n", "\u{a0}"] {
            assert_eq!(clean_note_text(raw), Err(NoteError::Empty), "{raw:?}");
        }
    }

    #[test]
    fn cleaning_keeps_the_content_and_drops_only_noise() {
        let raw = "\r\n\r\n  Título  \r\nlinha com espaço   \r\n\r\n\r\n\r\n- item\r\n\r\n";
        assert_eq!(
            clean_note_text(raw).unwrap(),
            "  Título\nlinha com espaço\n\n- item"
        );
    }

    #[test]
    fn a_huge_note_is_refused() {
        let raw = "a".repeat(MAX_NOTE_CHARS + 1);
        assert_eq!(clean_note_text(&raw), Err(NoteError::TooLong));
        assert!(clean_note_text(&"a".repeat(MAX_NOTE_CHARS)).is_ok());
    }

    #[test]
    fn a_short_first_line_is_the_title() {
        assert_eq!(
            note_title("Arquitetura do módulo\n\nTexto longo."),
            "Arquitetura do módulo"
        );
        assert_eq!(note_title("## Decisões\n\nCorpo"), "Decisões");
    }

    #[test]
    fn a_sentence_is_not_a_title_and_the_content_gives_one() {
        let text =
            "O prazo de carência para internações é de 180 dias contados da adesão ao plano.";
        let title = note_title(text);
        assert!(title.ends_with('…') && title.chars().count() <= DERIVED_TITLE_CHARS + 1);
        assert!(text.starts_with(title.trim_end_matches('…')));
        assert_eq!(note_title("Fim de semana."), "Fim de semana.");
    }
}
