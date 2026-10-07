use nlmx_application::ports::{Chunker, TokenCounter};
use nlmx_domain::{
    document::BoundingBox,
    ingestion::{Block, BlockKind, ChunkPolicy, PageBox, StructuredDocument},
};

use crate::{HeuristicTokenCounter, StructuralChunker, sha256_hex, split};

/// One token per word keeps the arithmetic in these tests obvious.
struct WordCounter;
impl TokenCounter for WordCounter {
    fn count(&self, text: &str) -> u32 {
        text.split_whitespace().count() as u32
    }
}

fn bbox(top: f32) -> BoundingBox {
    BoundingBox {
        left: 72.0,
        top,
        right: 500.0,
        bottom: top + 12.0,
    }
}

fn para(text: &str, page: u32, section: &[&str]) -> Block {
    Block {
        kind: BlockKind::Paragraph,
        text: text.into(),
        page,
        boxes: vec![PageBox {
            page,
            bbox: bbox(100.0),
        }],
        section_path: section.iter().map(|s| s.to_string()).collect(),
    }
}

fn heading(text: &str, level: u8, parents: &[&str]) -> Block {
    Block {
        kind: BlockKind::Heading { level },
        text: text.into(),
        page: 1,
        boxes: vec![],
        section_path: parents.iter().map(|s| s.to_string()).collect(),
    }
}

/// `n` sentences of five words each.
fn sentences(prefix: &str, n: usize) -> String {
    (0..n)
        .map(|i| format!("{prefix} frase número {i} aqui."))
        .collect::<Vec<_>>()
        .join(" ")
}

fn policy(target: u32, max: u32, overlap: u32, min: u32) -> ChunkPolicy {
    ChunkPolicy {
        target_tokens: target,
        max_tokens: max,
        overlap_tokens: overlap,
        min_tokens: min,
    }
}

fn chunk(blocks: Vec<Block>, policy: ChunkPolicy) -> Vec<nlmx_domain::ingestion::ChunkDraft> {
    StructuralChunker.chunk(&StructuredDocument { blocks }, &policy, &WordCounter)
}

#[test]
fn heuristic_token_counter_is_approximate_and_monotonic() {
    let counter = HeuristicTokenCounter;
    assert_eq!(counter.count(""), 0);
    assert_eq!(counter.count("um dois três"), 4); // 3 words × 4/3
    assert!(counter.count("anticonstitucionalissimamente") >= 7); // long word: chars / 4
    assert!(counter.count(&"palavra ".repeat(100)) > counter.count(&"palavra ".repeat(50)));
}

#[test]
fn small_sections_become_one_chunk_each_with_their_path() {
    let blocks = vec![
        heading("Contrato", 1, &[]),
        heading("Cláusula 1", 2, &["Contrato"]),
        para("Carência de 180 dias.", 1, &["Contrato", "Cláusula 1"]),
        para("Vale para consultas.", 1, &["Contrato", "Cláusula 1"]),
        heading("Cláusula 2", 2, &["Contrato"]),
        para("Cobertura hospitalar.", 2, &["Contrato", "Cláusula 2"]),
    ];
    let chunks = chunk(blocks, ChunkPolicy::default());
    assert_eq!(chunks.len(), 2);
    assert_eq!(
        chunks[0].text,
        "Carência de 180 dias.\n\nVale para consultas."
    );
    assert_eq!(
        chunks[0].section().as_deref(),
        Some("Contrato > Cláusula 1")
    );
    assert_eq!((chunks[0].index, chunks[1].index), (0, 1));
    assert_eq!((chunks[1].page_start, chunks[1].page_end), (2, 2));
    assert!(
        !chunks.iter().any(|c| c.text.contains("Cláusula")),
        "headings are not in chunk text"
    );
}

#[test]
fn long_sections_respect_the_target_and_overlap_within_the_section() {
    // 20 sentences × 5 words; target 30 → several chunks; overlap 5 → one sentence carried over.
    let blocks = vec![
        para(&sentences("A", 20), 1, &["S"]),
        para(&sentences("B", 4), 2, &["Outra"]),
    ];
    let chunks = chunk(blocks, policy(30, 40, 5, 3));
    let section_s: Vec<_> = chunks.iter().filter(|c| c.section_path == ["S"]).collect();
    assert!(section_s.len() >= 3);
    for c in &section_s {
        assert!(c.token_count <= 40, "max respected: {}", c.token_count);
    }
    for pair in section_s.windows(2) {
        let last_sentence = split::sentences(&pair[0].text).pop().unwrap();
        assert!(
            pair[1].text.starts_with(&last_sentence),
            "overlap: {:?} → {:?}",
            pair[0].text,
            pair[1].text
        );
    }
    let other = chunks.iter().find(|c| c.section_path == ["Outra"]).unwrap();
    assert!(
        other.text.starts_with("B frase número 0"),
        "no overlap across sections: {}",
        other.text
    );
}

#[test]
fn oversized_blocks_are_split_by_sentences_then_words() {
    let giant_sentence = format!("{} fim.", "palavra ".repeat(120));
    let blocks = vec![para(&giant_sentence, 1, &[])];
    let chunks = chunk(blocks, policy(50, 60, 0, 1));
    assert!(chunks.len() >= 2);
    assert!(
        chunks.iter().all(|c| c.token_count <= 60),
        "{:?}",
        chunks.iter().map(|c| c.token_count).collect::<Vec<_>>()
    );
    let rebuilt: Vec<&str> = chunks
        .iter()
        .flat_map(|c| c.text.split_whitespace())
        .collect();
    assert_eq!(
        rebuilt.len(),
        121,
        "no words lost or duplicated without overlap"
    );
}

#[test]
fn a_small_remainder_is_merged_into_the_previous_chunk() {
    let blocks = vec![
        para(&sentences("A", 6), 1, &["S"]),
        para("Curto.", 1, &["S"]),
    ];
    // 30 tokens then a 1-token remainder below min_tokens = 5.
    let chunks = chunk(blocks, policy(30, 40, 0, 5));
    assert_eq!(chunks.len(), 1);
    assert!(chunks[0].text.ends_with("\n\nCurto."));
}

#[test]
fn chunks_keep_pages_and_boxes_of_their_blocks() {
    let mut cross_page = para("Começa numa página e termina na outra.", 1, &[]);
    cross_page.boxes.push(PageBox {
        page: 2,
        bbox: bbox(60.0),
    });
    let chunks = chunk(vec![cross_page], ChunkPolicy::default());
    assert_eq!((chunks[0].page_start, chunks[0].page_end), (1, 2));
    assert_eq!(
        chunks[0].boxes.iter().map(|b| b.page).collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn hashes_and_output_are_deterministic() {
    let blocks = vec![para(&sentences("A", 30), 1, &["S"])];
    let a = chunk(blocks.clone(), policy(40, 60, 8, 5));
    let b = chunk(blocks, policy(40, 60, 8, 5));
    assert_eq!(a, b);
    for c in &a {
        assert_eq!(c.content_hash, sha256_hex(&c.text));
        assert_eq!(c.content_hash.len(), 64);
    }
    assert_eq!(
        sha256_hex("abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn sentence_splitting_skips_abbreviations_and_numbers() {
    assert_eq!(
        split::sentences(
            "O Dr. A. Souza pagou R$ 1.500. Depois saiu! Custa 180. Conforme art. 5 vale. Fim"
        ),
        [
            "O Dr. A. Souza pagou R$ 1.500.",
            "Depois saiu!",
            "Custa 180.",
            "Conforme art. 5 vale.",
            "Fim"
        ]
        .map(String::from)
    );
}
