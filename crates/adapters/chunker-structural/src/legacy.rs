//! The chunking algorithm as it was before the generic core, kept only to prove the new
//! wrapper produces identical drafts.

use nlmx_application::ports::TokenCounter;
use nlmx_domain::ingestion::{
    Block, BlockKind, ChunkDraft, ChunkPolicy, PageBox, StructuredDocument,
};

use crate::{sha256_hex, split};

/// A piece of a block (the whole block, or a sentence/word run of a large one).
#[derive(Debug, Clone)]
struct Unit {
    block: usize,
    text: String,
    tokens: u32,
    page_start: u32,
    page_end: u32,
    boxes: Vec<PageBox>,
}

pub fn legacy_chunk(
    document: &StructuredDocument,
    policy: &ChunkPolicy,
    tokens: &dyn TokenCounter,
) -> Vec<ChunkDraft> {
    {
        let mut chunks = Vec::new();
        for (section_path, blocks) in sections(&document.blocks) {
            let units = blocks
                .iter()
                .flat_map(|(index, block)| units_of(*index, block, policy, tokens))
                .collect::<Vec<_>>();
            chunk_section(&units, &section_path, policy, tokens, &mut chunks);
        }
        for (index, chunk) in chunks.iter_mut().enumerate() {
            chunk.index = index as u32;
        }
        chunks
    }
}

/// A section path and its blocks (with their index in the document).
type Section<'a> = (Vec<String>, Vec<(usize, &'a Block)>);

/// Consecutive non-heading blocks sharing a section path. Headings only define the path.
fn sections(blocks: &[Block]) -> Vec<Section<'_>> {
    let mut sections: Vec<Section<'_>> = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        if matches!(block.kind, BlockKind::Heading { .. }) || block.text.trim().is_empty() {
            continue;
        }
        match sections.last_mut() {
            Some((path, members)) if *path == block.section_path => members.push((index, block)),
            _ => sections.push((block.section_path.clone(), vec![(index, block)])),
        }
    }
    sections
}

fn units_of(
    index: usize,
    block: &Block,
    policy: &ChunkPolicy,
    tokens: &dyn TokenCounter,
) -> Vec<Unit> {
    let (page_start, page_end) = page_range(&block.boxes, block.page);
    let unit = |text: String| Unit {
        block: index,
        tokens: tokens.count(&text),
        text,
        page_start,
        page_end,
        boxes: block.boxes.clone(),
    };
    if tokens.count(&block.text) <= policy.max_tokens {
        return vec![unit(block.text.clone())];
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
        .map(unit)
        .collect()
}

fn page_range(boxes: &[PageBox], fallback: u32) -> (u32, u32) {
    let start = boxes.iter().map(|b| b.page).min().unwrap_or(fallback);
    let end = boxes.iter().map(|b| b.page).max().unwrap_or(fallback);
    (start, end)
}

fn chunk_section(
    units: &[Unit],
    section_path: &[String],
    policy: &ChunkPolicy,
    tokens: &dyn TokenCounter,
    out: &mut Vec<ChunkDraft>,
) {
    let section_start = out.len();
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

    // A small remainder joins the previous chunk of the same section when it fits.
    let new_part = &current[current.len() - fresh..];
    let new_tokens: u32 = new_part.iter().map(|u| u.tokens).sum();
    if new_tokens < policy.min_tokens && out.len() > section_start {
        let previous = out.last_mut().expect("section has a chunk");
        if previous.token_count + new_tokens <= policy.max_tokens {
            let mut merged = units_of_chunk(previous);
            merged.extend(new_part.iter().cloned());
            *previous = build(&merged, section_path, tokens);
            return;
        }
    }
    out.push(build(&current, section_path, tokens));
}

/// Re-expresses an existing chunk as a single unit so a remainder can be appended to it.
fn units_of_chunk(chunk: &ChunkDraft) -> Vec<Unit> {
    vec![Unit {
        block: usize::MAX,
        text: chunk.text.clone(),
        tokens: chunk.token_count,
        page_start: chunk.page_start,
        page_end: chunk.page_end,
        boxes: chunk.boxes.clone(),
    }]
}

/// The trailing sentences (or words) of `text` that fit in `overlap_tokens`.
fn overlap_unit(
    text: &str,
    last: &Unit,
    policy: &ChunkPolicy,
    tokens: &dyn TokenCounter,
) -> Option<Unit> {
    if policy.overlap_tokens == 0 {
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
        page_start: last.page_end,
        page_end: last.page_end,
        boxes: last
            .boxes
            .iter()
            .filter(|b| b.page == last.page_end)
            .copied()
            .collect(),
    })
}

fn build(units: &[Unit], section_path: &[String], tokens: &dyn TokenCounter) -> ChunkDraft {
    let mut text = String::new();
    let mut boxes: Vec<PageBox> = Vec::new();
    for (i, unit) in units.iter().enumerate() {
        if i > 0 {
            // Pieces of the same block flow as one paragraph; different blocks stay separate.
            text.push_str(if units[i - 1].block == unit.block {
                " "
            } else {
                "\n\n"
            });
        }
        text.push_str(&unit.text);
        for b in &unit.boxes {
            if !boxes.contains(b) {
                boxes.push(*b);
            }
        }
    }
    ChunkDraft {
        index: 0,
        token_count: tokens.count(&text),
        page_start: units.iter().map(|u| u.page_start).min().unwrap_or(1),
        page_end: units.iter().map(|u| u.page_end).max().unwrap_or(1),
        section_path: section_path.to_vec(),
        boxes,
        content_hash: sha256_hex(&text),
        text,
    }
}
