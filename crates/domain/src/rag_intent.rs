//! What kind of question the user asked: a regular question (answered by search), a request
//! to explain a whole document, or a question about a numbered section.

use crate::retrieval::fold;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryIntent {
    Regular,
    /// "Explique este documento.", "Summarize this document".
    Overview,
    /// "Quais são os principais pontos da seção 3?" — the section number, e.g. "3" or "3.2".
    Section(String),
}

const SECTION_WORDS: &[&str] = &[
    "secao", "secoes", "capitulo", "clausula", "item", "parte", "topico", "section", "chapter",
    "clause", "part",
];
const OVERVIEW_VERBS: &[&str] = &[
    "explique",
    "explica",
    "explicar",
    "explicacao",
    "resuma",
    "resumir",
    "resumo",
    "sintetize",
    "sintese",
    "apresente",
    "descreva",
    "summarize",
    "summarise",
    "summary",
    "explain",
    "describe",
    "overview",
];
const DOCUMENT_WORDS: &[&str] = &[
    "documento",
    "arquivo",
    "pdf",
    "relatorio",
    "texto",
    "document",
    "file",
    "report",
];

impl QueryIntent {
    pub fn parse(question: &str) -> Self {
        let text = fold(question);
        if let Some(label) = section_label(&text) {
            return Self::Section(label);
        }
        let words: Vec<&str> = text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        let has = |set: &[&str]| words.iter().any(|w| set.contains(w));
        let about = text.contains("do que trata")
            || text.contains("do que se trata")
            || text.contains("sobre o que e")
            || text.contains("what is this")
            || text.contains("what's this");
        if has(DOCUMENT_WORDS) && (has(OVERVIEW_VERBS) || about) {
            Self::Overview
        } else {
            Self::Regular
        }
    }
}

/// "secao 3", "capitulo 3.2", "section III" → "3", "3.2", "3".
fn section_label(text: &str) -> Option<String> {
    let mut rest = text;
    while !rest.is_empty() {
        let start = rest.find(|c: char| c.is_alphanumeric())?;
        rest = &rest[start..];
        let end = rest
            .find(|c: char| !c.is_alphanumeric())
            .unwrap_or(rest.len());
        let (word, after) = rest.split_at(end);
        rest = after;
        if !SECTION_WORDS.contains(&word) {
            continue;
        }
        let candidate = after.trim_start_matches([' ', 'n', 'º', '°', '.', '#']);
        let candidate = candidate.trim_start();
        let token: String = candidate
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '.')
            .collect();
        let token = token.trim_end_matches('.');
        if token.is_empty() {
            continue;
        }
        if token
            .split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        {
            return Some(token.to_string());
        }
        if let Some(n) = roman(token) {
            return Some(n.to_string());
        }
    }
    None
}

fn roman(token: &str) -> Option<u32> {
    let value = |c| match c {
        'i' => Some(1),
        'v' => Some(5),
        'x' => Some(10),
        'l' => Some(50),
        'c' => Some(100),
        _ => None,
    };
    let digits: Vec<u32> = token.chars().map(value).collect::<Option<_>>()?;
    if digits.is_empty() || digits.len() > 6 {
        return None;
    }
    let mut total = 0;
    for (i, d) in digits.iter().enumerate() {
        if digits.get(i + 1).is_some_and(|next| next > d) {
            total -= *d as i32;
        } else {
            total += *d as i32;
        }
    }
    (total > 0).then_some(total as u32)
}

/// Whether a chunk's section title belongs to `label` ("3" matches "3. Prazos" and
/// "3.1 Carência", not "13. Anexos" or "30 dias").
pub fn section_matches(section: &str, label: &str) -> bool {
    // A section path may be "2. Coberturas > 3. Prazos"; the last heading counts, and so do its parents.
    section.split(" > ").any(|heading| {
        let heading = heading.trim();
        let Some(rest) = heading.strip_prefix(label) else {
            return false;
        };
        match rest.chars().next() {
            None => true,
            Some('.') => rest[1..]
                .chars()
                .next()
                .is_none_or(|c| c.is_ascii_digit() || c.is_whitespace() || !c.is_alphanumeric()),
            Some(c) => c.is_whitespace() || c == ')' || c == '-' || c == '–',
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_intents() {
        let s = |n: &str| QueryIntent::Section(n.into());
        let cases = [
            ("Explique este documento.", QueryIntent::Overview),
            ("Resuma o documento", QueryIntent::Overview),
            ("Do que trata este PDF?", QueryIntent::Overview),
            ("faça um resumo do relatório", QueryIntent::Overview),
            ("Summarize this document", QueryIntent::Overview),
            ("Explain this file, please", QueryIntent::Overview),
            ("Quais são os principais pontos da seção 3?", s("3")),
            ("O que diz a SECAO 3.2?", s("3.2")),
            ("resuma o capítulo III", s("3")),
            ("What does section 4 say?", s("4")),
            ("cláusula nº 7", s("7")),
            ("Qual a carência?", QueryIntent::Regular),
            ("Explique a carência", QueryIntent::Regular),
            ("a seção de prazos", QueryIntent::Regular),
            (
                "Quanto tempo de carência para parte das consultas?",
                QueryIntent::Regular,
            ),
        ];
        for (question, intent) in cases {
            assert_eq!(QueryIntent::parse(question), intent, "{question}");
        }
    }

    #[test]
    fn section_titles_match_by_number() {
        assert!(section_matches("3. Prazos", "3"));
        assert!(section_matches("3.1 Carência", "3"));
        assert!(section_matches("3 Prazos", "3"));
        assert!(section_matches("3) Prazos", "3"));
        assert!(section_matches("2. Coberturas > 3. Prazos", "3"));
        assert!(section_matches("3.2. Exceções", "3.2"));
        assert!(!section_matches("13. Anexos", "3"));
        assert!(!section_matches("30 dias", "3"));
        assert!(!section_matches("3.2 Exceções", "3.1"));
        assert!(!section_matches("Prazos", "3"));
    }
}
