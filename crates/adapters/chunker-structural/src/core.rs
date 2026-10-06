//! The chunking algorithm shared by every format: blocks grouped by section path, packed to a
//! token target with sentence overlap, each chunk located by merging the locations of what it
//! holds. It knows nothing about pages, lines or chapters beyond `SourceLocation::merge` and
//! `SourceLocation::trailing`.

use nlmx_application::ports::TokenCounter;
use nlmx_domain::{
    ingestion::ChunkPolicy,
    parsed::{ContentBlock, ContentKind},
    source::SourceLocation,
};

use crate::split;

/// A chunk before it is given its document, index and hash.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CoreChunk {
    pub text: String,
    pub token_count: u32,
    pub section_path: Vec<String>,
    pub location: SourceLocation,
}

/// A block with the path of the section it is in.
pub(crate) type Item<'a> = (&'a [String], &'a ContentBlock);

/// A piece of a block (the whole block, or a sentence/word/line run of a large one).
#[derive(Debug, Clone)]
struct Unit {
    block: usize,
    text: String,
    tokens: u32,
    location: SourceLocation,
    /// A list item of a non-paged document (consecutive ones are joined with one newline).
    list_item: bool,
    /// A run of lines of a code block or table (pieces of it are joined with a newline).
    by_lines: bool,
}

/// Chunks `items` (blocks in reading order). `paged` is true for PDF, which keeps its
/// historical behaviour: every block is separated by a blank line and large blocks are always
/// cut by sentences.
pub(crate) fn chunk(
    items: &[Item<'_>],
    paged: bool,
    policy: &ChunkPolicy,
    tokens: &dyn TokenCounter,
) -> Vec<CoreChunk> {
    let mut chunks = Vec::new();
    for (section_path, blocks) in groups(items) {
        let units = blocks
            .iter()
            .flat_map(|(index, block)| units_of(*index, block, paged, policy, tokens))
            .collect::<Vec<_>>();
        chunk_group(&units, &section_path, policy, tokens, &mut chunks);
    }
    chunks
}

/// A section path and its blocks (with their index in the document).
type Group<'a> = (Vec<String>, Vec<(usize, &'a ContentBlock)>);

/// Consecutive non-heading blocks sharing a section path (and able to share a location: an
/// EPUB chapter never joins another). Headings only define the path.
fn groups<'a>(items: &[Item<'a>]) -> Vec<Group<'a>> {
    let mut groups: Vec<Group<'a>> = Vec::new();
    for (index, (path, block)) in items.iter().enumerate() {
        if matches!(block.kind, ContentKind::Heading { .. }) || block.text.trim().is_empty() {
            continue;
        }
        match groups.last_mut() {
            Some((group_path, members))
                if group_path.as_slice() == *path
                    && members.last().is_some_and(|(_, last)| {
                        last.location.merge(&block.location).is_some()
                    }) =>
            {
                members.push((index, block));
            }
            _ => groups.push((path.to_vec(), vec![(index, block)])),
        }
    }
    groups
}

fn units_of(
    index: usize,
    block: &ContentBlock,
    paged: bool,
    policy: &ChunkPolicy,
    tokens: &dyn TokenCounter,
) -> Vec<Unit> {
    let list_item = !paged && matches!(block.kind, ContentKind::ListItem { .. });
    let unit = |text: String, by_lines: bool| Unit {
        block: index,
        tokens: tokens.count(&text),
        text,
        location: block.location.clone(),
        list_item,
        by_lines,
    };
    if tokens.count(&block.text) <= policy.max_tokens {
        return vec![unit(block.text.clone(), false)];
    }
    let line_based = !paged
        && matches!(
            block.kind,
            ContentKind::CodeBlock { .. } | ContentKind::Table { .. }
        );
    if line_based {
        return split::lines(&block.text, policy.target_tokens, tokens)
            .into_iter()
            .map(|piece| unit(piece, true))
            .collect();
    }
    split::sentences(&block.text)
        .into_iter()
        .flat_map(|sentence| {
            if tokens.count(&sentence) <= policy.max_tokens {
                vec![sentence]
            } else {
                split::words(&sentence, policy.target_tokens, tokens)
            }
        })
        .map(|piece| unit(piece, false))
        .collect()
}

fn chunk_group(
    units: &[Unit],
    section_path: &[String],
    policy: &ChunkPolicy,
    tokens: &dyn TokenCounter,
    out: &mut Vec<CoreChunk>,
) {
    let group_start = out.len();
    let mut current: Vec<Unit> = Vec::new();
    // Units in `current` that are new content (not carried over as overlap).
    let mut fresh = 0usize;

    for unit in units {
        let current_tokens: u32 = current.iter().map(|u| u.tokens).sum();
        if fresh > 0 && current_tokens + unit.tokens > policy.target_tokens {
            let chunk = build(&current, section_path, tokens);
            let overlap = overlap_unit(
                &chunk.text,
                current.last().expect("non-empty"),
                policy,
                tokens,
            );
            out.push(chunk);
            current = overlap.into_iter().collect();
            fresh = 0;
        }
        current.push(unit.clone());
        fresh += 1;
    }
    if fresh == 0 {
        return;
    }

    // A small remainder joins the previous chunk of the same group when it fits.
    let new_part = &current[current.len() - fresh..];
    let new_tokens: u32 = new_part.iter().map(|u| u.tokens).sum();
    if new_tokens < policy.min_tokens && out.len() > group_start {
        let previous = out.last_mut().expect("group has a chunk");
        if previous.token_count + new_tokens <= policy.max_tokens {
            let mut merged = vec![Unit {
                block: usize::MAX,
                text: previous.text.clone(),
                tokens: previous.token_count,
                location: previous.location.clone(),
                list_item: false,
                by_lines: false,
            }];
            merged.extend(new_part.iter().cloned());
            *previous = build(&merged, section_path, tokens);
            return;
        }
    }
    out.push(build(&current, section_path, tokens));
}

/// The trailing sentences (or words) of `text` that fit in `overlap_tokens`. Runs of code or
/// table lines are not repeated.
fn overlap_unit(
    text: &str,
    last: &Unit,
    policy: &ChunkPolicy,
    tokens: &dyn TokenCounter,
) -> Option<Unit> {
    if policy.overlap_tokens == 0 || last.by_lines {
        return None;
    }
    let sentences = split::sentences(text.rsplit("\n\n").next().unwrap_or(text));
    let mut tail: Vec<&str> = Vec::new();
    let mut used = 0;
    for sentence in sentences.iter().rev() {
        let n = tokens.count(sentence);
        if used + n > policy.overlap_tokens {
            break;
        }
        used += n;
        tail.push(sentence);
    }
    let overlap = if tail.is_empty() {
        split::last_words(text, policy.overlap_tokens, tokens)
    } else {
        tail.reverse();
        tail.join(" ")
    };
    (!overlap.is_empty()).then(|| Unit {
        block: last.block,
        tokens: tokens.count(&overlap),
        text: overlap,
        location: last.location.trailing(),
        list_item: false,
        by_lines: false,
    })
}

fn build(units: &[Unit], section_path: &[String], tokens: &dyn TokenCounter) -> CoreChunk {
    let mut text = String::new();
    let mut location: Option<SourceLocation> = None;
    for (i, unit) in units.iter().enumerate() {
        if i > 0 {
            let previous = &units[i - 1];
            text.push_str(if previous.block == unit.block {
                // Pieces of the same block flow as one paragraph (or keep their lines).
                if unit.by_lines { "\n" } else { " " }
            } else if previous.list_item && unit.list_item {
                "\n"
            } else {
                "\n\n"
            });
        }
        text.push_str(&unit.text);
        location = Some(match location {
            None => unit.location.clone(),
            // Units of one group always merge; keep what we have if one ever did not.
            Some(acc) => acc.merge(&unit.location).unwrap_or(acc),
        });
    }
    CoreChunk {
        token_count: tokens.count(&text),
        section_path: section_path.to_vec(),
        location: location.expect("a chunk has at least one unit"),
        text,
    }
}
