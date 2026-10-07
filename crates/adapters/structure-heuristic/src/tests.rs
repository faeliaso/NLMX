use nlmx_application::ports::StructureAnalyzer;
use nlmx_domain::{
    document::{BoundingBox, TextSpan},
    ingestion::{BlockKind, PageLayout},
};

use crate::{HeuristicStructureAnalyzer, lines::is_page_number, normalize, starts_list_item};

const PAGE_H: f32 = 792.0;

/// A span whose baseline is `baseline` points from the top of the page.
fn span(text: &str, left: f32, baseline: f32, size: f32, bold: bool) -> TextSpan {
    TextSpan {
        text: text.into(),
        bbox: BoundingBox {
            left,
            top: baseline - size * 0.9,
            right: left + text.chars().count() as f32 * size * 0.5,
            bottom: baseline + size * 0.2,
        },
        font_name: if bold { "Helvetica-Bold" } else { "Helvetica" }.into(),
        font_size: size,
        bold,
        italic: false,
    }
}

fn page(number: u32, spans: Vec<TextSpan>) -> PageLayout {
    PageLayout {
        number,
        width: 612.0,
        height: PAGE_H,
        has_text: !spans.is_empty(),
        char_count: 0,
        spans,
    }
}

fn texts(pages: &[PageLayout]) -> Vec<(BlockKind, String, Vec<String>)> {
    HeuristicStructureAnalyzer
        .analyze(pages)
        .blocks
        .into_iter()
        .map(|b| (b.kind, b.text, b.section_path))
        .collect()
}

#[test]
fn normalizes_ligatures_spaces_and_invisible_characters() {
    // ligature ﬁ, double NBSP → one space, ligature ﬃ, soft hyphen and zero-width space dropped.
    assert_eq!(
        normalize("su\u{FB01}ciente\u{00A0}\u{00A0}e\u{FB03}ci\u{00AD}ente\u{200B}\t "),
        "suficiente efficiente"
    );
    assert_eq!(normalize("  \n "), "");
}

#[test]
fn recognizes_page_numbers_and_list_markers() {
    for text in [
        "12",
        "- 12 -",
        "Página 3 de 10",
        "Page 3 of 10",
        "pág. 4",
        "3/10",
    ] {
        assert!(is_page_number(text), "{text}");
    }
    for text in ["2026 foi um ano", "Capítulo 3", "12 meses"] {
        assert!(!is_page_number(text), "{text}");
    }
    for text in [
        "• Consultas",
        "- Exames",
        "1. Primeiro",
        "2) Segundo",
        "a) letra",
        "(b) letra",
    ] {
        assert!(starts_list_item(text), "{text}");
    }
    for text in ["A. Souza assinou", "1.5 milhão", "Texto normal", "-5 graus"] {
        assert!(!starts_list_item(text), "{text}");
    }
}

#[test]
fn builds_headings_sections_paragraphs_and_lists() {
    let pages = [page(
        1,
        vec![
            span("Contrato", 72.0, 80.0, 20.0, true),
            span("Cláusula 1", 72.0, 120.0, 14.0, true),
            span("O período de carên-", 72.0, 145.0, 11.0, false),
            span("cia é de 180 dias.", 72.0, 159.0, 11.0, false),
            span("Cláusula 2", 72.0, 190.0, 14.0, true),
            span("• Consultas", 72.0, 215.0, 11.0, false),
            span("• Exames de", 72.0, 229.0, 11.0, false),
            span("imagem", 84.0, 243.0, 11.0, false),
            span("Texto após um espaço grande.", 72.0, 300.0, 11.0, false),
        ],
    )];
    let blocks = texts(&pages);
    let expected: Vec<(BlockKind, &str, Vec<&str>)> = vec![
        (BlockKind::Heading { level: 1 }, "Contrato", vec![]),
        (
            BlockKind::Heading { level: 2 },
            "Cláusula 1",
            vec!["Contrato"],
        ),
        (
            BlockKind::Paragraph,
            "O período de carência é de 180 dias.",
            vec!["Contrato", "Cláusula 1"],
        ),
        (
            BlockKind::Heading { level: 2 },
            "Cláusula 2",
            vec!["Contrato"],
        ),
        (
            BlockKind::ListItem,
            "• Consultas",
            vec!["Contrato", "Cláusula 2"],
        ),
        (
            BlockKind::ListItem,
            "• Exames de imagem",
            vec!["Contrato", "Cláusula 2"],
        ),
        (
            BlockKind::Paragraph,
            "Texto após um espaço grande.",
            vec!["Contrato", "Cláusula 2"],
        ),
    ];
    let expected: Vec<_> = expected
        .into_iter()
        .map(|(k, t, s)| {
            (
                k,
                t.to_string(),
                s.into_iter().map(String::from).collect::<Vec<_>>(),
            )
        })
        .collect();
    assert_eq!(blocks, expected);
}

#[test]
fn a_line_ending_soft_hyphen_also_joins_words() {
    let pages = [page(
        1,
        vec![
            span("perío\u{00AD}", 72.0, 100.0, 11.0, false),
            span("do de teste.", 72.0, 114.0, 11.0, false),
        ],
    )];
    assert_eq!(texts(&pages)[0].1, "período de teste.");
}

#[test]
fn keeps_hyphens_that_are_not_line_breaks() {
    let pages = [page(
        1,
        vec![
            span("Atendimento pós-", 72.0, 100.0, 11.0, false),
            span("Operatório e bem-estar.", 72.0, 114.0, 11.0, false),
        ],
    )];
    // Next line starts with an uppercase letter: not a hyphenated word.
    assert_eq!(
        texts(&pages)[0].1,
        "Atendimento pós- Operatório e bem-estar."
    );
}

#[test]
fn removes_repeated_headers_footers_and_page_numbers() {
    let pages: Vec<_> = (1..=3)
        .map(|n| {
            page(
                n,
                vec![
                    span("ACME — Relatório Anual", 72.0, 40.0, 9.0, false),
                    span(
                        &format!("Conteúdo da página {n}."),
                        72.0,
                        300.0,
                        11.0,
                        false,
                    ),
                    span(&format!("Página {n} de 3"), 260.0, 770.0, 9.0, false),
                ],
            )
        })
        .collect();
    let blocks = texts(&pages);
    let all: Vec<&str> = blocks.iter().map(|(_, t, _)| t.as_str()).collect();
    assert_eq!(
        all,
        [
            "Conteúdo da página 1.",
            "Conteúdo da página 2.",
            "Conteúdo da página 3."
        ]
    );
}

#[test]
fn margin_text_that_does_not_repeat_is_kept() {
    let pages = [
        page(
            1,
            vec![
                span("Título no topo", 72.0, 40.0, 11.0, false),
                span("Corpo.", 72.0, 300.0, 11.0, false),
            ],
        ),
        page(2, vec![span("Outro corpo.", 72.0, 300.0, 11.0, false)]),
        page(3, vec![span("Mais corpo.", 72.0, 300.0, 11.0, false)]),
    ];
    assert_eq!(texts(&pages)[0].1, "Título no topo");
}

#[test]
fn paragraph_continues_across_pages_until_final_punctuation() {
    let pages = [
        page(
            1,
            vec![
                span("Introdução", 72.0, 80.0, 14.0, true),
                span("O prazo termina após", 72.0, 700.0, 11.0, false),
            ],
        ),
        page(
            2,
            vec![
                span("cento e oitenta dias.", 72.0, 80.0, 11.0, false),
                span("Novo parágrafo.", 72.0, 120.0, 11.0, false),
            ],
        ),
    ];
    let blocks = HeuristicStructureAnalyzer.analyze(&pages).blocks;
    assert_eq!(blocks[1].text, "O prazo termina após cento e oitenta dias.");
    assert_eq!(blocks[1].page, 1);
    assert_eq!(
        blocks[1].boxes.iter().map(|b| b.page).collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(blocks[2].text, "Novo parágrafo.");
}

#[test]
fn merges_spans_on_the_same_line_in_reading_order() {
    let pages = [page(
        1,
        vec![
            span("segunda parte", 200.0, 100.0, 11.0, false),
            span("Primeira parte,", 72.0, 100.0, 11.0, false),
            span("negrito", 300.0, 100.0, 11.0, true),
        ],
    )];
    let blocks = HeuristicStructureAnalyzer.analyze(&pages).blocks;
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].text, "Primeira parte, segunda parte negrito");
    let b = blocks[0].boxes[0].bbox;
    assert!(b.left == 72.0 && b.right > 300.0);
}

#[test]
fn analysis_is_deterministic() {
    let pages = [page(
        1,
        vec![
            span("Título", 72.0, 80.0, 16.0, true),
            span("Corpo do texto.", 72.0, 110.0, 11.0, false),
        ],
    )];
    assert_eq!(
        HeuristicStructureAnalyzer.analyze(&pages),
        HeuristicStructureAnalyzer.analyze(&pages)
    );
}
