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

/// Splits into runs of whole lines (joined by `\n`) of at most `max_tokens` each; a single line
/// over the limit is cut into word runs. Used for code and tables, which must not be cut at
/// sentence ends.
pub fn lines(text: &str, max_tokens: u32, tokens: &dyn TokenCounter) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let push_current = |current: &mut String, out: &mut Vec<String>| {
        let piece = std::mem::take(current);
        let piece = piece.trim_end_matches('\n');
        if !piece.trim().is_empty() {
            out.push(piece.to_string());
        }
    };
    for line in text.lines() {
        if tokens.count(line) > max_tokens {
            push_current(&mut current, &mut out);
            out.extend(words(line, max_tokens, tokens));
            continue;
        }
        let candidate = if current.is_empty() {
            line.to_string()
        } else {
            format!("{current}\n{line}")
        };
        if !current.is_empty() && tokens.count(&candidate) > max_tokens {
            push_current(&mut current, &mut out);
            current = line.to_string();
        } else {
            current = candidate;
        }
    }
    push_current(&mut current, &mut out);
    out
}

#[cfg(test)]
mod lines_tests {
    use super::*;

    struct WordCounter;
    impl TokenCounter for WordCounter {
        fn count(&self, text: &str) -> u32 {
            text.split_whitespace().count() as u32
        }
    }

    #[test]
    fn whole_lines_are_packed_and_long_lines_cut_by_words() {
        let text = "a b c\nd e f\ng h i\n\nj k l m n o p";
        let pieces = lines(text, 6, &WordCounter);
        assert_eq!(pieces, ["a b c\nd e f", "g h i", "j k l m n o", "p"]);
        assert!(lines("", 5, &WordCounter).is_empty());
        assert!(lines("\n\n  \n", 5, &WordCounter).is_empty());
    }
}
