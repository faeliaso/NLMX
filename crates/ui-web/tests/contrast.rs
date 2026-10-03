//! WCAG 2.2 contrast checks for the design tokens in `apps/desktop/ui/styles/tokens.css`,
//! evaluated separately for the light and dark values of each `light-dark()` token.

use std::collections::HashMap;

const TOKENS: &str = include_str!("../../../apps/desktop/ui/styles/tokens.css");

/// Minimum ratios: 4.5 for text, 3.0 for non-text UI boundaries (WCAG 1.4.3 / 1.4.11).
const TEXT: f64 = 4.5;
const UI: f64 = 3.0;

const PAIRS: &[(&str, &str, f64)] = &[
    ("fg", "bg", TEXT),
    ("fg", "bg-subtle", TEXT),
    ("fg", "surface-raised", TEXT),
    ("fg", "surface-hover", TEXT),
    ("fg-muted", "bg", TEXT),
    ("fg-muted", "bg-subtle", TEXT),
    ("fg-muted", "surface", TEXT),
    ("fg-muted", "surface-raised", TEXT),
    ("accent-fg", "accent", TEXT),
    ("accent-fg", "accent-hover", TEXT),
    ("accent-fg", "accent-active", TEXT),
    ("accent-text", "bg", TEXT),
    ("accent-text", "surface", TEXT),
    ("accent-text", "accent-soft", TEXT),
    ("fg", "accent-soft", TEXT),
    ("success-fg", "bg", TEXT),
    ("success-fg", "success-soft", TEXT),
    ("warning-fg", "bg", TEXT),
    ("warning-fg", "warning-soft", TEXT),
    ("danger-fg", "bg", TEXT),
    ("danger-fg", "surface", TEXT),
    ("danger-fg", "danger-soft", TEXT),
    ("danger-fill-fg", "danger-fill", TEXT),
    ("info-fg", "bg", TEXT),
    ("info-fg", "info-soft", TEXT),
    ("border-strong", "surface", UI),
    ("border-strong", "bg", UI),
    ("accent", "bg", UI),
];

type Rgb = (f64, f64, f64);

/// Parses `--color-NAME: light-dark(#RRGGBB, #RRGGBB);` lines. Tokens with alpha are skipped.
fn tokens() -> HashMap<String, (Rgb, Rgb)> {
    TOKENS
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("--color-")?;
            let (name, value) = rest.split_once(':')?;
            let inner = value
                .trim()
                .strip_prefix("light-dark(")?
                .split(')')
                .next()?;
            let (light, dark) = inner.split_once(',')?;
            Some((name.to_string(), (hex(light.trim())?, hex(dark.trim())?)))
        })
        .collect()
}

fn hex(value: &str) -> Option<Rgb> {
    let digits = value.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let channel = |i: usize| {
        u8::from_str_radix(&digits[i..i + 2], 16)
            .ok()
            .map(f64::from)
    };
    Some((channel(0)?, channel(2)?, channel(4)?))
}

fn luminance((r, g, b): Rgb) -> f64 {
    let linear = |c: f64| {
        let c = c / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

fn ratio(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[test]
fn every_pair_meets_wcag_aa_in_both_themes() {
    let tokens = tokens();
    let mut failures = Vec::new();
    for &(fg, bg, min) in PAIRS {
        let (fg_l, fg_d) = tokens
            .get(fg)
            .unwrap_or_else(|| panic!("missing token {fg}"));
        let (bg_l, bg_d) = tokens
            .get(bg)
            .unwrap_or_else(|| panic!("missing token {bg}"));
        for (theme, a, b) in [("light", fg_l, bg_l), ("dark", fg_d, bg_d)] {
            let r = ratio(*a, *b);
            if r < min {
                failures.push(format!("{theme}: {fg} on {bg} = {r:.2} (< {min})"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "contrast failures:\n{}",
        failures.join("\n")
    );
}

#[test]
fn parser_finds_the_palette() {
    assert!(tokens().len() >= 30, "token parser found too few colors");
}
