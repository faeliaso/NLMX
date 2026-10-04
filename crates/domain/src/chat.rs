//! Conversations: questions, answers and the sources each answer was given.

use crate::{
    ingestion::{DocumentId, PageBox},
    vectors::ChunkId,
};

pub type ConversationId = i64;
pub type MessageId = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// Where an assistant message is in its life: generating, or how it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageStatus {
    Streaming,
    Answered,
    /// Nothing relevant in the documents (or no document chosen for an overview).
    NotFound,
    /// The model's guardrails declined.
    Refused,
    /// Stopped by the user; the content is the partial answer.
    Cancelled,
    /// Generation failed; see `error`.
    Failed,
}

impl MessageStatus {
    pub fn is_final(self) -> bool {
        self != Self::Streaming
    }
}

/// What a conversation answers from (ADR 0009).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationScope {
    /// Open conversation with the language model; the library is never searched.
    Free,
    /// Every document.
    Library,
    Document(DocumentId),
}

impl ConversationScope {
    /// How answers to new questions are produced.
    pub fn grounding(self) -> AnswerGrounding {
        match self {
            Self::Free => AnswerGrounding::Free,
            Self::Library | Self::Document(_) => AnswerGrounding::Documents,
        }
    }

    pub fn document(self) -> Option<DocumentId> {
        match self {
            Self::Document(id) => Some(id),
            Self::Free | Self::Library => None,
        }
    }
}

/// What an answer was generated from. Kept per answer: changing the scope of a conversation
/// does not change earlier answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerGrounding {
    /// Retrieved passages, with citations and the relevance gate.
    Documents,
    /// The language model alone, without sources.
    Free,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    pub id: ConversationId,
    pub title: Option<String>,
    pub scope: ConversationScope,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationSummary {
    pub id: ConversationId,
    pub title: Option<String>,
    pub updated_at: String,
    pub messages: u32,
}

/// A passage given to the model for an answer (a snapshot: it survives re-indexing).
#[derive(Debug, Clone, PartialEq)]
pub struct MessageSource {
    /// `[n]` in the answer.
    pub n: u32,
    pub cited: bool,
    pub document_id: DocumentId,
    /// `None` once the chunk was replaced by a re-index.
    pub chunk_id: Option<ChunkId>,
    pub document_title: String,
    pub page_start: u32,
    pub page_end: u32,
    pub section: Option<String>,
    pub label: String,
    pub quote: String,
    pub bboxes: Vec<PageBox>,
}

/// A `[página N]` reference in an answer: opens the document at that page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagePageRef {
    pub page: u32,
    pub document_id: DocumentId,
    /// The source (`n`) covering that page, whose boxes are highlighted.
    pub source: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub id: MessageId,
    pub conversation_id: ConversationId,
    pub role: Role,
    pub content: String,
    pub status: MessageStatus,
    /// Assistant messages only.
    pub grounding: Option<AnswerGrounding>,
    pub error: Option<String>,
    /// Assistant messages only, by `n`.
    pub sources: Vec<MessageSource>,
    /// `[página N]` references of the answer, in order of appearance (unique pages).
    pub page_refs: Vec<MessagePageRef>,
    pub created_at: String,
}
