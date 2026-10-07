//! The generic core, through `StructuralChunker`, produces exactly the drafts of the original
//! algorithm (kept in `legacy.rs`) for PDF-shaped input.

use nlmx_application::ports::Chunker;
use nlmx_domain::{
    document::BoundingBox,
    ingestion::{Block, BlockKind, ChunkPolicy, PageBox, StructuredDocument},
};

use crate::{HeuristicTokenCounter, StructuralChunker, legacy::legacy_chunk};

/// A small deterministic generator (no dependency).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
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

fn sentences(rng: &mut Rng, count: u64) -> String {
    (0..count)
        .map(|i| match rng.below(4) {
            0 => format!("Veja o art. {i} do contrato aqui."),
            1 => format!("Frase número {i} do trecho, com algumas palavras."),
            2 => format!("Será que a frase {i} termina?"),
            _ => format!("Texto {i}."),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn document(seed: u64) -> StructuredDocument {
    let mut rng = Rng(seed);
    let mut blocks = Vec::new();
    let mut page = 1;
    let titles = ["Introdução", "Escopo", "Prazos", "Escopo"];
    let mut path: Vec<String> = Vec::new();
    for _ in 0..rng.below(14) + 1 {
        if rng.below(7) == 0 {
            page += 1;
        }
        match rng.below(6) {
            0 | 1 => {
                // Headings, with titles that repeat so equal paths show up.
                let level = rng.below(2) as u8 + 1;
                let title = titles[rng.below(4) as usize].to_string();
                path.truncate(level as usize - 1);
                blocks.push(Block {
                    kind: BlockKind::Heading { level },
                    text: title.clone(),
                    page,
                    boxes: vec![],
                    section_path: path.clone(),
                });
                path.push(title);
            }
            kind => {
                let count = [1, 2, 4, 9, 30, 90][rng.below(6) as usize];
                let boxes = match rng.below(4) {
                    0 => vec![],
                    1 => vec![
                        PageBox {
                            page,
                            bbox: bbox(100.0),
                        },
                        PageBox {
                            page: page + 1,
                            bbox: bbox(120.0),
                        },
                    ],
                    _ => vec![PageBox {
                        page,
                        bbox: bbox(100.0 + rng.below(300) as f32),
                    }],
                };
                blocks.push(Block {
                    kind: if kind == 5 {
                        BlockKind::ListItem
                    } else {
                        BlockKind::Paragraph
                    },
                    text: if rng.below(9) == 0 {
                        "   ".into()
                    } else {
                        sentences(&mut rng, count)
                    },
                    page,
                    boxes,
                    section_path: path.clone(),
                });
            }
        }
    }
    StructuredDocument { blocks }
}

#[test]
fn the_wrapper_matches_the_original_algorithm() {
    let policies = [
        ChunkPolicy::default(),
        ChunkPolicy {
            target_tokens: 40,
            max_tokens: 60,
            overlap_tokens: 10,
            min_tokens: 15,
        },
        ChunkPolicy {
            target_tokens: 25,
            max_tokens: 30,
            overlap_tokens: 0,
            min_tokens: 8,
        },
        ChunkPolicy {
            target_tokens: 120,
            max_tokens: 200,
            overlap_tokens: 30,
            min_tokens: 60,
        },
    ];
    let mut chunks_compared = 0;
    for seed in 0..400 {
        let document = document(seed);
        for policy in &policies {
            let old = legacy_chunk(&document, policy, &HeuristicTokenCounter);
            let new = StructuralChunker.chunk(&document, policy, &HeuristicTokenCounter);
            assert_eq!(new, old, "seed {seed}, policy {policy:?}");
            chunks_compared += new.len();
        }
    }
    assert!(
        chunks_compared > 2_000,
        "the generator produced {chunks_compared} chunks"
    );
}

#[test]
fn the_same_section_path_across_repeated_headings_is_one_group() {
    // Two headings with the same title give blocks with equal paths; the original merged them.
    let block = |kind, text: &str, path: &[&str]| Block {
        kind,
        text: text.into(),
        page: 1,
        boxes: vec![],
        section_path: path.iter().map(|s| s.to_string()).collect(),
    };
    let document = StructuredDocument {
        blocks: vec![
            block(BlockKind::Heading { level: 1 }, "Escopo", &[]),
            block(BlockKind::Paragraph, "Primeiro parágrafo.", &["Escopo"]),
            block(BlockKind::Heading { level: 1 }, "Escopo", &[]),
            block(BlockKind::Paragraph, "Segundo parágrafo.", &["Escopo"]),
        ],
    };
    let policy = ChunkPolicy::default();
    let old = legacy_chunk(&document, &policy, &HeuristicTokenCounter);
    let new = StructuralChunker.chunk(&document, &policy, &HeuristicTokenCounter);
    assert_eq!(new, old);
    assert_eq!(new.len(), 1);
    assert_eq!(new[0].text, "Primeiro parágrafo.\n\nSegundo parágrafo.");
}
