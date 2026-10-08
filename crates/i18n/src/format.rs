use crate::Locale;

fn decimal_separator(locale: Locale) -> char {
    match locale {
        Locale::En => '.',
        Locale::PtBr | Locale::Es => ',',
    }
}

/// `value` with `decimals` fraction digits and the locale's decimal separator.
pub fn format_number(locale: Locale, value: f64, decimals: usize) -> String {
    format!("{value:.decimals$}").replace('.', &decimal_separator(locale).to_string())
}

/// A whole number with the locale's thousands separator: `8.192` (pt-BR, es), `8,192` (en).
pub fn format_integer(locale: Locale, value: u64) -> String {
    let separator = match locale {
        Locale::En => ',',
        Locale::PtBr | Locale::Es => '.',
    };
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(separator);
        }
        out.push(digit);
    }
    out
}

/// `12%` (`12 %` in Spanish) from a 0..=1 ratio.
pub fn format_percent(locale: Locale, ratio: f64) -> String {
    let n = format_number(locale, ratio * 100.0, 0);
    match locale {
        Locale::Es => format!("{n} %"),
        _ => format!("{n}%"),
    }
}

/// Decimal (SI) sizes: `812 B`, `1,5 MB`, `4.2 GB`.
pub fn format_bytes(locale: Locale, bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    let decimals = if unit == 0 || value >= 100.0 { 0 } else { 1 };
    format!("{} {}", format_number(locale, value, decimals), UNITS[unit])
}
