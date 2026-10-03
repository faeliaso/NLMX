//! Answer generation (language model) concepts.

/// Whether the on-device language model can be used right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanguageModelStatus {
    Available,
    /// The model exists but its terms of use have not been accepted on this Mac.
    LicenseRequired,
    /// This Mac or system can never run it as is (macOS < 27, Intel or Rosetta, `fm` missing).
    Incompatible {
        reason: String,
    },
    /// The system is compatible but the model can't be used now; the reason is suitable for display.
    Unavailable {
        kind: UnavailableKind,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnavailableKind {
    /// Apple Intelligence is turned off in System Settings.
    AppleIntelligenceDisabled,
    /// The hardware does not support Apple Intelligence.
    DeviceNotEligible,
    /// The model is still downloading or preparing; trying later may work.
    ModelNotReady,
    Other,
}

impl LanguageModelStatus {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    /// The model may become available without user action (e.g. it is still downloading).
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Unavailable {
                kind: UnavailableKind::ModelNotReady,
                ..
            }
        )
    }
}

/// What the model can take. The context window covers instructions, prompt and answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LlmCapabilities {
    pub context_tokens: u32,
}

/// One generation: instructions (system) and the user turn. Provider-neutral.
#[derive(Debug, Clone, PartialEq)]
pub struct GenerationRequest {
    /// Instructions. Never contains document text.
    pub system: String,
    pub user: String,
    pub temperature: f32,
    /// Upper bound for the answer, in tokens.
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishReason {
    Completed,
    /// Stopped at `max_tokens`.
    Length,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Generation {
    pub text: String,
    pub finish: FinishReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmError {
    /// The model cannot be used on this Mac (reason suitable for display).
    Unavailable(String),
    /// The model's terms have not been accepted (`sudo fm license`).
    LicenseRequired,
    /// The model's safety guardrails declined to answer.
    Refused(String),
    /// The prompt does not fit the context window.
    ContextTooLong {
        tokens: u32,
        limit: u32,
    },
    Timeout,
    /// Unexpected answer from the model process.
    Protocol(String),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "modelo de linguagem indisponível: {reason}"),
            Self::LicenseRequired => f.write_str(
                "os termos do Apple Foundation Models não foram aceitos (sudo fm license)",
            ),
            Self::Refused(reason) => write!(f, "o modelo recusou responder: {reason}"),
            Self::ContextTooLong { tokens, limit } => {
                write!(f, "o pedido tem {tokens} tokens; o limite é {limit}")
            }
            Self::Timeout => f.write_str("o modelo de linguagem não respondeu a tempo"),
            Self::Protocol(message) => write!(f, "resposta inesperada do modelo: {message}"),
        }
    }
}

impl std::error::Error for LlmError {}
