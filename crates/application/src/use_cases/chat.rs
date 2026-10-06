//! The Chat: conversations saved with their questions, answers and sources. Answers come from
//! the documents (RAG engine, with citations) or, in a free conversation, from the language
//! model alone (ADR 0009); text streams through `on_token` while it is generated.

use std::sync::Arc;

use nlmx_domain::{
    chat::{
        AnswerGrounding, Conversation, ConversationId, ConversationScope, ConversationSummary,
        Message, MessageId, MessagePageRef, MessageSource, MessageStatus, Role,
    },
    generation::{LanguageModelStatus, UnavailableKind},
    ingestion::DocumentId,
    rag_intent::QueryIntent,
};

use crate::{
    ports::{CancelFlag, ConversationRepository, FinishedAnswer, StorageError},
    services::{
        free_chat::{FreeAnswer, FreeChat},
        rag::{
            AnswerStatus, HistoryTurn, RagAnswer, RagEngine, RagError, RagOptions, context::Source,
        },
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatError {
    NotFound,
    /// The message is not an answer waiting to be generated.
    NotPending,
    /// Only an answer not found in the documents can be answered without them.
    NotAnswerableFreely,
    EmptyQuestion,
    Storage(String),
}

impl std::fmt::Display for ChatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("conversa ou mensagem não encontrada"),
            Self::NotPending => f.write_str("esta resposta não está aguardando geração"),
            Self::NotAnswerableFreely => f.write_str(
                "só uma resposta não encontrada nos documentos pode ser refeita sem eles",
            ),
            Self::EmptyQuestion => f.write_str("escreva uma pergunta"),
            Self::Storage(m) => f.write_str(m),
        }
    }
}

impl From<StorageError> for ChatError {
    fn from(e: StorageError) -> Self {
        Self::Storage(e.message)
    }
}

/// Earlier exchanges used to understand follow-up questions.
const HISTORY_TURNS: usize = 3;
/// Earlier exchanges given to the model in a free conversation (the oldest may not fit).
const FREE_HISTORY_TURNS: usize = 6;
const TITLE_CHARS: usize = 60;

pub struct ChatService {
    pub conversations: Arc<dyn ConversationRepository>,
    pub rag: Arc<RagEngine>,
    pub free: Arc<FreeChat>,
    pub options: RagOptions,
}

impl ChatService {
    /// The most recent conversation, or a new free one.
    pub async fn current(&self) -> Result<Conversation, ChatError> {
        match self.conversations.recent(1).await?.first() {
            Some(summary) => self
                .conversations
                .get(summary.id)
                .await?
                .ok_or(ChatError::NotFound),
            None => Ok(self.conversations.create(ConversationScope::Free).await?),
        }
    }

    pub async fn conversation(&self, id: ConversationId) -> Result<Conversation, ChatError> {
        self.conversations.get(id).await?.ok_or(ChatError::NotFound)
    }

    /// A new conversation with the given scope.
    pub async fn start(&self, scope: ConversationScope) -> Result<Conversation, ChatError> {
        Ok(self.conversations.create(scope).await?)
    }

    pub async fn recent(&self, limit: u32) -> Result<Vec<ConversationSummary>, ChatError> {
        Ok(self.conversations.recent(limit).await?)
    }

    pub async fn messages(&self, id: ConversationId) -> Result<Vec<Message>, ChatError> {
        Ok(self.conversations.messages(id).await?)
    }

    pub async fn message(&self, id: MessageId) -> Result<Message, ChatError> {
        self.conversations
            .message(id)
            .await?
            .ok_or(ChatError::NotFound)
    }

    /// Applies to the next questions; earlier answers keep how they were generated.
    pub async fn set_scope(
        &self,
        id: ConversationId,
        scope: ConversationScope,
    ) -> Result<(), ChatError> {
        Ok(self.conversations.set_scope(id, scope).await?)
    }

    pub async fn delete(&self, id: ConversationId) -> Result<(), ChatError> {
        Ok(self.conversations.delete(id).await?)
    }

    /// Saves the question and an answer placeholder (`Streaming`); `answer` fills it.
    pub async fn ask(
        &self,
        conversation: ConversationId,
        question: &str,
    ) -> Result<(MessageId, MessageId), ChatError> {
        let question = question.trim();
        if question.is_empty() {
            return Err(ChatError::EmptyQuestion);
        }
        let current = self.conversation(conversation).await?;
        let user = self
            .conversations
            .add_message(
                conversation,
                Role::User,
                question,
                MessageStatus::Answered,
                None,
            )
            .await?;
        let assistant = self
            .conversations
            .add_message(
                conversation,
                Role::Assistant,
                "",
                MessageStatus::Streaming,
                Some(current.scope.grounding()),
            )
            .await?;
        if current.title.is_none() {
            let mut title: String = question.chars().take(TITLE_CHARS).collect();
            if question.chars().count() > TITLE_CHARS {
                title.push('…');
            }
            self.conversations.set_title(conversation, &title).await?;
        }
        Ok((user, assistant))
    }

    /// Generates the answer of a `Streaming` message and saves it with its sources.
    pub async fn answer(
        &self,
        message: MessageId,
        on_token: &(dyn Fn(&str) + Send + Sync),
        cancel: CancelFlag,
    ) -> Result<Message, ChatError> {
        let pending = self.message(message).await?;
        if pending.role != Role::Assistant || pending.status != MessageStatus::Streaming {
            return Err(ChatError::NotPending);
        }
        let conversation = self.conversation(pending.conversation_id).await?;
        let messages = self.conversations.messages(conversation.id).await?;
        let position = messages
            .iter()
            .position(|m| m.id == message)
            .ok_or(ChatError::NotFound)?;
        let earlier = &messages[..position];
        let question = earlier
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| m.content.clone())
            .ok_or(ChatError::NotFound)?;
        let earlier = &earlier[..earlier.len().saturating_sub(1)];

        let (content, status, error, sources, page_refs) = match pending.grounding {
            Some(AnswerGrounding::Free) => {
                let history = history(earlier, FREE_HISTORY_TURNS, false);
                let result = self
                    .free
                    .answer(&question, &history, &self.options, on_token, cancel)
                    .await;
                free_outcome(result)
            }
            Some(AnswerGrounding::Documents) | None => {
                let history = history(earlier, HISTORY_TURNS, true);
                let mut options = self.options.clone();
                // "Explique este documento" after leaving a single document for the whole
                // library still means the document the conversation was about.
                let document = conversation.scope.document().or_else(|| {
                    (QueryIntent::parse(&question) == QueryIntent::Overview)
                        .then(|| last_cited_document(earlier))
                        .flatten()
                });
                if let Some(document) = document {
                    options.retriever.filter.documents = Some(vec![document]);
                }
                let result = self
                    .rag
                    .converse(&question, &history, &options, on_token, cancel)
                    .await;
                outcome(result)
            }
        };
        self.conversations
            .finish_message(
                message,
                FinishedAnswer {
                    content: &content,
                    status,
                    error: error.as_deref(),
                    sources: &sources,
                    page_refs: &page_refs,
                },
            )
            .await?;
        self.message(message).await
    }

    /// Clears an answer so that `answer` generates it again, the same way as before.
    pub async fn regenerate(&self, message: MessageId) -> Result<(), ChatError> {
        let m = self.message(message).await?;
        if m.role != Role::Assistant || m.status == MessageStatus::Streaming {
            return Err(ChatError::NotPending);
        }
        let grounding = m.grounding.unwrap_or(AnswerGrounding::Documents);
        Ok(self.conversations.reset_message(message, grounding).await?)
    }

    /// "Responder sem os documentos": an answer not found in the documents is generated again
    /// by the model alone. The conversation's scope does not change.
    pub async fn answer_freely(&self, message: MessageId) -> Result<(), ChatError> {
        let m = self.message(message).await?;
        if m.role != Role::Assistant
            || m.status != MessageStatus::NotFound
            || m.grounding == Some(AnswerGrounding::Free)
        {
            return Err(ChatError::NotAnswerableFreely);
        }
        Ok(self
            .conversations
            .reset_message(message, AnswerGrounding::Free)
            .await?)
    }
}

/// The one document cited by the latest answer that cited something; `None` when that answer
/// cited several documents or nothing was cited yet.
fn last_cited_document(messages: &[Message]) -> Option<DocumentId> {
    let cited: Vec<DocumentId> = messages
        .iter()
        .rev()
        .filter(|m| m.role == Role::Assistant && m.grounding != Some(AnswerGrounding::Free))
        .map(|m| {
            let mut documents: Vec<DocumentId> = m
                .sources
                .iter()
                .filter(|s| s.cited)
                .map(|s| s.document_id)
                .collect();
            documents.dedup();
            documents
        })
        .find(|documents| !documents.is_empty())?;
    match cited.as_slice() {
        [document] => Some(*document),
        _ => None,
    }
}

/// The last `limit` question/answer pairs that ended with an answer, most recent last.
/// `documents_only` drops free answers: they say nothing about the documents, and a follow-up
/// rewritten from them would search for the wrong thing.
fn history(messages: &[Message], limit: usize, documents_only: bool) -> Vec<HistoryTurn> {
    let mut turns = Vec::new();
    let mut question: Option<&str> = None;
    for m in messages {
        match m.role {
            Role::User => question = Some(&m.content),
            Role::Assistant => {
                let skip = documents_only && m.grounding == Some(AnswerGrounding::Free);
                if let (Some(q), MessageStatus::Answered, false) = (question.take(), m.status, skip)
                {
                    turns.push(HistoryTurn {
                        question: q.to_string(),
                        answer: m.content.clone(),
                    });
                }
            }
        }
    }
    let skip = turns.len().saturating_sub(limit);
    turns.into_iter().skip(skip).collect()
}

fn sources(sources: &[Source], cited: &[usize]) -> Vec<MessageSource> {
    sources
        .iter()
        .map(|s| MessageSource {
            n: s.n as u32,
            cited: cited.contains(&s.n),
            document_id: s.document_id,
            chunk_id: Some(s.chunk_id),
            document_title: s.title.clone(),
            page_start: s.page_start,
            page_end: s.page_end,
            section: s.section.clone(),
            label: s.label.clone(),
            quote: s.content.clone(),
            bboxes: s.bboxes.clone(),
            reference: s.provenance.reference.clone(),
            document_name: s.provenance.document_name.clone(),
        })
        .collect()
}

type Outcome = (
    String,
    MessageStatus,
    Option<String>,
    Vec<MessageSource>,
    Vec<MessagePageRef>,
);

fn outcome(result: Result<RagAnswer, RagError>) -> Outcome {
    match result {
        Ok(answer) => {
            let cited: Vec<usize> = answer.citations.iter().map(|c| c.n).collect();
            let status = message_status(answer.status);
            let error = refusal(status, &answer.warnings);
            let page_refs = answer
                .page_refs
                .iter()
                .map(|r| MessagePageRef {
                    page: r.page,
                    document_id: r.document_id,
                    source: r.source.map(|n| n as u32),
                })
                .collect();
            (
                answer.answer,
                status,
                error,
                sources(&answer.sources, &cited),
                page_refs,
            )
        }
        Err(RagError::ModelUnavailable { status, sources: s }) => (
            String::new(),
            MessageStatus::Failed,
            Some(describe(&status)),
            sources(&s, &[]),
            Vec::new(),
        ),
        Err(e) => (
            String::new(),
            MessageStatus::Failed,
            Some(e.to_string()),
            Vec::new(),
            Vec::new(),
        ),
    }
}

/// A free answer has no sources nor page references.
fn free_outcome(result: Result<FreeAnswer, RagError>) -> Outcome {
    match result {
        Ok(free) => {
            let status = message_status(free.status);
            let error = refusal(status, &free.warnings);
            (free.answer, status, error, Vec::new(), Vec::new())
        }
        Err(e) => outcome(Err(e)),
    }
}

fn message_status(status: AnswerStatus) -> MessageStatus {
    match status {
        AnswerStatus::Answered => MessageStatus::Answered,
        AnswerStatus::NotFound => MessageStatus::NotFound,
        AnswerStatus::Refused => MessageStatus::Refused,
        AnswerStatus::Cancelled => MessageStatus::Cancelled,
    }
}

/// The guardrails' reason, kept as the error of a refused answer.
fn refusal(status: MessageStatus, warnings: &[String]) -> Option<String> {
    (status == MessageStatus::Refused)
        .then(|| warnings.last().cloned())
        .flatten()
}

/// What the user can do about an unavailable model.
pub fn describe(status: &LanguageModelStatus) -> String {
    match status {
        LanguageModelStatus::Available => "O modelo de linguagem não respondeu.".into(),
        LanguageModelStatus::LicenseRequired => "Os termos do Apple Foundation Models ainda não foram aceitos neste Mac. No Terminal, execute: sudo fm license".into(),
        LanguageModelStatus::NotInstalled => "O NLMX não encontrou o fm neste Mac. Verifique se a sua versão do macOS e os componentes do Apple Foundation Models estão disponíveis.".into(),
        LanguageModelStatus::Incompatible { reason } => reason.clone(),
        LanguageModelStatus::Unavailable { kind, reason } => match kind {
            UnavailableKind::AppleIntelligenceDisabled => "O Apple Intelligence está desativado. Ative-o em Ajustes do Sistema › Apple Intelligence e Siri.".into(),
            UnavailableKind::DeviceNotEligible => "Este Mac não é compatível com o Apple Intelligence.".into(),
            UnavailableKind::ModelNotReady => "O macOS ainda está preparando o modelo. Tente novamente em alguns minutos.".into(),
            UnavailableKind::Other => reason.clone(),
        },
    }
}
