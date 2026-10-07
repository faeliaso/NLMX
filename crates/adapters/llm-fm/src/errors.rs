//! One mapping from what `fm` reports (exit codes, stderr, SSE errors) to `LlmError` and
//! from availability messages to `UnavailableKind`.

use nlmx_domain::generation::{LlmError, UnavailableKind};

/// `fm` exits with this status until its terms are accepted (`sudo fm license`).
pub const EXIT_LICENSE_NOT_ACCEPTED: i32 = 69;

/// Removes ANSI escape sequences (`ESC [ ... letter`) that `fm` may emit.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Cleans `fm`'s "Error: ..." prefix and colours.
pub fn clean(text: &str) -> String {
    let text = strip_ansi(text);
    let text = text.trim();
    text.strip_prefix("Error:")
        .unwrap_or(text)
        .trim()
        .to_string()
}

/// An error message from a generation (SSE `event: error`, stderr of `fm respond`).
pub fn from_message(message: &str) -> LlmError {
    let message = clean(message);
    let lower = message.to_lowercase();
    if lower.contains("guardrail") || lower.contains("unsafe") || lower.contains("refus") {
        LlmError::Refused(message)
    } else if lower.contains("license") || lower.contains("terms") {
        LlmError::LicenseRequired
    } else if lower.contains("context") && (lower.contains("exceed") || lower.contains("window"))
        || lower.contains("too long")
    {
        LlmError::ContextTooLong {
            tokens: 0,
            limit: crate::SYSTEM_CONTEXT_TOKENS,
        }
    } else if !matches!(classify(&lower), UnavailableKind::Other) {
        LlmError::Unavailable(message)
    } else {
        LlmError::Protocol(message)
    }
}

/// Exit status of an `fm` command that failed, with its stderr.
pub fn from_exit(code: Option<i32>, stderr: &str) -> LlmError {
    match code {
        Some(EXIT_LICENSE_NOT_ACCEPTED) => LlmError::LicenseRequired,
        None => LlmError::Unavailable("o processo do fm foi interrompido".into()),
        Some(code) => {
            let message = clean(stderr);
            if message.is_empty() {
                LlmError::Protocol(format!("o fm terminou com o código {code}"))
            } else {
                from_message(&message)
            }
        }
    }
}

/// Why the model is unavailable, from `fm available`'s message (English or Portuguese).
pub fn classify(message: &str) -> UnavailableKind {
    // API reasons may come as camelCase ("appleIntelligenceNotEnabled").
    let m = split_camel(message).to_lowercase();
    let any = |keys: &[&str]| keys.iter().any(|k| m.contains(k));
    if any(&[
        "not eligible",
        "not supported",
        "unsupported",
        "não é compatível",
        "incompat",
    ]) {
        UnavailableKind::DeviceNotEligible
    } else if any(&[
        "not ready",
        "downloading",
        "preparing",
        "não está pronto",
        "baixando",
    ]) {
        UnavailableKind::ModelNotReady
    } else if any(&[
        "apple intelligence",
        "not enabled",
        "turned off",
        "disabled",
        "desativad",
    ]) {
        UnavailableKind::AppleIntelligenceDisabled
    } else {
        UnavailableKind::Other
    }
}

fn split_camel(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut previous_lower = false;
    for c in text.chars() {
        if c.is_uppercase() && previous_lower {
            out.push(' ');
        }
        previous_lower = c.is_lowercase();
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_availability_messages() {
        let cases = [
            (
                "Apple Intelligence is not enabled",
                UnavailableKind::AppleIntelligenceDisabled,
            ),
            (
                "System model unavailable: appleIntelligenceNotEnabled",
                UnavailableKind::AppleIntelligenceDisabled,
            ),
            (
                "Device not eligible for Apple Intelligence",
                UnavailableKind::DeviceNotEligible,
            ),
            (
                "System model unavailable: deviceNotEligible",
                UnavailableKind::DeviceNotEligible,
            ),
            (
                "Model not ready (downloading)",
                UnavailableKind::ModelNotReady,
            ),
            (
                "System model unavailable: modelNotReady",
                UnavailableKind::ModelNotReady,
            ),
            ("something odd", UnavailableKind::Other),
        ];
        for (text, kind) in cases {
            assert_eq!(classify(text), kind, "{text}");
        }
    }

    #[test]
    fn maps_generation_failures() {
        assert!(matches!(
            from_message("Error: The model's safety guardrails were triggered."),
            LlmError::Refused(m) if m == "The model's safety guardrails were triggered."
        ));
        assert_eq!(from_exit(Some(69), ""), LlmError::LicenseRequired);
        assert!(matches!(
            from_exit(Some(1), "Error: Exceeded model context window size"),
            LlmError::ContextTooLong { .. }
        ));
        assert!(
            matches!(from_exit(Some(1), "\u{1b}[31mboom\u{1b}[0m"), LlmError::Protocol(m) if m == "boom")
        );
        assert!(matches!(from_exit(None, ""), LlmError::Unavailable(_)));
        assert!(matches!(
            from_message("Apple Intelligence is not enabled"),
            LlmError::Unavailable(_)
        ));
    }
}
