//! Splitting text into sentences and token-bounded word runs.

use nlmx_application::ports::TokenCounter;

/// Abbreviations (lowercase, without the period) that do not end a sentence.
const ABBREVIATIONS: &[&str] = &[
    "dr", "dra", "sr", "sra", "srta", "prof", "profa", "eng", "av", "art", "arts", "inc", "par",
    "cap", "fig", "vol", "pág", "pag", "p", "pp", "ex", "obs", "cf", "etc", "nº", "n", "no",
    "ltda", "s.a", "mr", "mrs", "ms", "vs", "e.g", "i.e",
];

/// Sentences end at `.`, `!`, `?` or `…` followed by whitespace — except after initials ("A.") and
/// common abbreviations ("Dr.", "art.", "nº.").
pub fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (i, &(byte, c)) in chars.iter().enumerate() {
        if !matches!(c, '.' | '!' | '?' | '…') {
            continue;
        }
        let next_is_space = chars.get(i + 1).is_some_and(|(_, n)| n.is_whitespace());
        if !next_is_space {
            continue;
        }
        let word_before: String = text[start..byte]
            .chars()
            .rev()
            .take_while(|c| !c.is_whitespace())
            .collect();
        let word_before: String = word_before.chars().rev().collect::<String>().to_lowercase();
        let abbreviation = c == '.'
            && (word_before.chars().count() == 1 && word_before.chars().all(char::is_alphabetic)
                || ABBREVIATIONS.contains(&word_before.as_str()));
        if abbreviation {
            continue;
        }
        let end = byte + c.len_utf8();
        push_trimmed(&mut out, &text[start..end]);
        start = end;
    }
    push_trimmed(&mut out, &text[start..]);
    out
}

fn push_trimmed(out: &mut Vec<String>, piece: &str) {
    let piece = piece.trim();
    if !piece.is_empty() {
        out.push(piece.to_string());
    }
}

/// Splits into runs of words of at most `max_tokens` each (a single huge word stays whole).
pub fn words(text: &str, max_tokens: u32, tokens: &dyn TokenCounter) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if !current.is_empty() && tokens.count(&candidate) > max_tokens {
            out.push(std::mem::take(&mut current));
            current = word.to_string();
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// The last words of `text` that fit in `max_tokens`.
pub fn last_words(text: &str, max_tokens: u32, tokens: &dyn TokenCounter) -> String {
    let mut tail: Vec<&str> = Vec::new();
    for word in text.split_whitespace().rev() {
        tail.push(word);
        let candidate: Vec<&str> = tail.iter().rev().copied().collect();
        if tokens.count(&candidate.join(" ")) > max_tokens {
            tail.pop();
            break;
        }
    }
    tail.reverse();
    tail.join(" ")
}
