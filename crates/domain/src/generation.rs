//! Answer generation (language model) concepts.

/// Whether the on-device language model can be used right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanguageModelStatus {
    Available,
    /// The model exists but its terms of use have not been accepted on this Mac.
    LicenseRequired,
    /// The system tool `fm` is not on this Mac (or is not executable).
    NotInstalled,
    /// This Mac or system can never run it as is (macOS < 27, Intel or Rosetta).
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

/// The language block as a literal, for `concat!` in prompt constants.
#[macro_export]
macro_rules! response_language_auto {
    () => {
        "RESPONSE LANGUAGE: AUTO — Respond primarily in the same language the user wrote the question in."
    };
}

/// The single language block of every prompt, shared as a literal so prompt constants can embed it.
pub const RESPONSE_LANGUAGE_AUTO: &str = response_language_auto!();

/// Which language the model answers in. Independent of the interface language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResponseLanguage {
    /// The language of the user's question.
    #[default]
    Auto,
}

impl ResponseLanguage {
    /// The language block to include once in each prompt's instructions.
    pub fn instruction(self) -> &'static str {
        match self {
            Self::Auto => RESPONSE_LANGUAGE_AUTO,
        }
    }
}

/// What the model can take. The context window covers instructions, prompt and answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LlmCapabilities {
    pub context_tokens: u32,
}

/// An earlier exchange of a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatTurn {
    pub user: String,
    pub assistant: String,
}

/// One generation: instructions (system), earlier turns and the user turn. Provider-neutral.
#[derive(Debug, Clone, PartialEq)]
pub struct GenerationRequest {
    /// Instructions. Never contains document text.
    pub system: String,
    /// Earlier turns, oldest first, sent as real conversation turns: given as text inside the
    /// user turn, the on-device model repeats earlier answers in new ones.
    pub history: Vec<ChatTurn>,
    pub user: String,
    pub temperature: f32,
    /// Upper bound for the answer, in tokens.
    pub max_tokens: u32,
}

impl GenerationRequest {
    /// The user turn with `history` written before it, for interfaces that take a single
    /// prompt (`fm respond`, `fm count-tokens`).
    pub fn flat_user(&self) -> String {
        if self.history.is_empty() {
            return self.user.clone();
        }
        let mut text = String::from("Conversa até aqui:\n");
        for turn in &self.history {
            text.push_str(&format!(
                "Usuário: {}\nAssistente: {}\n",
                turn.user, turn.assistant
            ));
        }
        text.push_str(&format!("\nMensagem atual do usuário:\n{}", self.user));
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_adds_the_prompt_and_the_answer() {
        let usage = GenerationUsage {
            prompt_tokens: 101,
            completion_tokens: 19,
        };
        assert_eq!(usage.total(), 120);
        assert!((usage.context_ratio() - 120.0 / 8192.0).abs() < 1e-12);
    }

    #[test]
    fn usage_does_not_overflow_with_extreme_values() {
        let usage = GenerationUsage {
            prompt_tokens: u32::MAX,
            completion_tokens: u32::MAX,
        };
        assert_eq!(usage.total(), u64::from(u32::MAX) * 2);
        assert!(usage.context_ratio().is_finite());
    }

    #[test]
    fn flat_user_writes_the_history_before_the_message() {
        let mut request = GenerationRequest {
            system: "s".into(),
            history: Vec::new(),
            user: "oi".into(),
            temperature: 0.5,
            max_tokens: 10,
        };
        assert_eq!(request.flat_user(), "oi");
        request.history.push(ChatTurn {
            user: "meu nome é Ana".into(),
            assistant: "Olá, Ana!".into(),
        });
        assert_eq!(
            request.flat_user(),
            "Conversa até aqui:\nUsuário: meu nome é Ana\nAssistente: Olá, Ana!\n\nMensagem atual do usuário:\noi"
        );
    }
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
    /// Token counts of the generation; `None` when cancelled or when they are unavailable.
    pub usage: Option<GenerationUsage>,
}

impl Generation {
    pub fn new(text: String, finish: FinishReason) -> Self {
        Self {
            text,
            finish,
            usage: None,
        }
    }
}

/// The on-device model's context window, measured on `fm serve` (prompts of ~8.1k tokens answer
/// correctly, ~8.5k fail with "transcript exceeded the model's context size") and shown by
/// `fm chat` as "… / 8.192". Single source for the RAG budget and for the status bar.
pub const MODEL_CONTEXT_WINDOW: u32 = 8192;

/// Tokens of one generation as the model counts them: everything it was given (instructions,
/// earlier turns, question) and what it produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

impl GenerationUsage {
    pub fn total(&self) -> u64 {
        u64::from(self.prompt_tokens) + u64::from(self.completion_tokens)
    }

    /// Share of the model's context window used (`0.01` = 1%).
    pub fn context_ratio(&self) -> f64 {
        self.total() as f64 / f64::from(MODEL_CONTEXT_WINDOW)
    }
}

/// The latest generation measured in the session: the conversation turn it answered and its usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastGeneration {
    /// 1-based position of the answer in its conversation.
    pub turn: u32,
    pub usage: GenerationUsage,
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
