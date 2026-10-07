//! Line numbers and paragraph boundaries over normalized (`\n`) text.

use std::ops::Range;

/// Byte offset to 1-based line number.
pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
        Self { starts }
    }

    /// The line the byte at `offset` is on.
    pub fn line_of(&self, offset: usize) -> u32 {
        self.starts.partition_point(|start| *start <= offset) as u32
    }
}

/// Byte ranges of the paragraphs (runs of non-blank lines), trimmed of outer whitespace.
pub fn paragraph_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut current: Option<Range<usize>> = None;
    let mut position = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches('\n');
        if content.trim().is_empty() {
            ranges.extend(current.take());
        } else {
            let end = position + content.len();
            match current.as_mut() {
                Some(range) => range.end = end,
                None => current = Some(position..end),
            }
        }
        position += line.len();
    }
    ranges.extend(current);
    ranges
        .into_iter()
        .map(|range| {
            let slice = &text[range.clone()];
            let start = range.start + (slice.len() - slice.trim_start().len());
            let end = range.end - (slice.len() - slice.trim_end().len());
            start..end
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_numbered_from_one() {
        let index = LineIndex::new("ab\ncd\n\nef");
        assert_eq!(index.line_of(0), 1);
        assert_eq!(index.line_of(2), 1, "the newline belongs to its line");
        assert_eq!(index.line_of(3), 2);
        assert_eq!(index.line_of(6), 3);
        assert_eq!(index.line_of(7), 4);
        assert_eq!(index.line_of(100), 4);
    }

    #[test]
    fn paragraphs_are_runs_of_non_blank_lines() {
        let text = "  um\ndois  \n\n\n  \ntrês\n";
        let found: Vec<_> = paragraph_ranges(text)
            .iter()
            .map(|r| &text[r.clone()])
            .collect();
        assert_eq!(found, ["um\ndois", "três"]);
        assert!(paragraph_ranges(" \n\t\n").is_empty());
        assert!(paragraph_ranges("").is_empty());
    }
}
