//! Chat: conversations with answers generated from the documents. The answer text streams
//! through a Tauri Channel (custom-protocol responses can't stream); these handlers render the
//! page, the question/answer turns, the final answer with citations and the source panel.

use std::sync::Arc;

use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    response::{Html, IntoResponse, Response},
};
use http::{HeaderMap, StatusCode};
use nlmx_application::use_cases::{ChatError, ChatService, describe_model_status};
use nlmx_domain::{
    chat::{Conversation, ConversationSummary, Message, MessageSource, MessageStatus, Role},
    ingestion::DocumentId,
};
use serde::Deserialize;

use crate::{
    AppState,
    error::UiError,
    markdown::{self, Citations},
    shell::{Section, page},
    status::LanguageModelView,
};

/// Suggestions shown in an empty conversation.
const SUGGESTIONS: &[&str] = &[
    "Explique este documento.",
    "Quais são os principais pontos da seção 3?",
];
const RECENT_CONVERSATIONS: u32 = 15;
const QUOTE_PREVIEW_CHARS: usize = 220;

pub struct DocumentOption {
    pub id: DocumentId,
    pub title: String,
    pub selected: bool,
}

pub struct RecentView {
    pub id: i64,
    pub title: String,
    pub current: bool,
}

pub struct SourceView {
    /// Opens the source in the viewer.
    pub url: String,
    pub n: u32,
    pub title: String,
    pub pages: String,
    pub section: String,
    pub preview: String,
}

pub struct AnswerView {
    pub id: i64,
    /// streaming | answered | not_found | refused | cancelled | failed
    pub status: &'static str,
    pub html: String,
    pub error: String,
    pub cited: Vec<SourceView>,
    pub consulted: Vec<SourceView>,
    pub copy_text: String,
}

impl AnswerView {
    pub fn streaming(&self) -> bool {
        self.status == "streaming"
    }
}

pub struct TurnView {
    pub question: String,
    pub answer: AnswerView,
}

#[derive(Template)]
#[template(path = "pages/sections/chat.html")]
struct ChatPage {
    conversation_id: i64,
    title: String,
    enabled: bool,
    documents: Vec<DocumentOption>,
    scope_title: String,
    recent: Vec<RecentView>,
    turns: Vec<TurnView>,
    suggestions: &'static [&'static str],
    /// Shown when the language model can't answer (license, Apple Intelligence off, ...).
    model_notice: Option<LanguageModelView>,
    /// The viewer, when the page opens with a document (`?view=`).
    viewer_html: String,
}

#[derive(Template)]
#[template(path = "components/chat_turn.html")]
struct TurnFragment {
    turn: TurnView,
}

#[derive(Template)]
#[template(path = "components/chat_answer.html")]
struct AnswerFragment {
    answer: AnswerView,
}

fn pages(start: u32, end: u32) -> String {
    if start == end {
        format!("p. {start}")
    } else {
        format!("pp. {start}–{end}")
    }
}

fn source_url(message_id: i64, s: &MessageSource) -> String {
    format!(
        "/viewer/{}?page={}&cite={message_id}-{}",
        s.document_id, s.page_start, s.n
    )
}

fn source_view(message_id: i64, s: &MessageSource) -> SourceView {
    let mut preview: String = s.quote.chars().take(QUOTE_PREVIEW_CHARS).collect();
    if s.quote.chars().count() > QUOTE_PREVIEW_CHARS {
        preview.push('…');
    }
    SourceView {
        url: source_url(message_id, s),
        n: s.n,
        title: s.document_title.clone(),
        pages: pages(s.page_start, s.page_end),
        section: s.section.clone().unwrap_or_default(),
        preview,
    }
}

pub fn answer_view(message: &Message) -> AnswerView {
    let status = match message.status {
        MessageStatus::Streaming => "streaming",
        MessageStatus::Answered => "answered",
        MessageStatus::NotFound => "not_found",
        MessageStatus::Refused => "refused",
        MessageStatus::Cancelled => "cancelled",
        MessageStatus::Failed => "failed",
    };
    let source = |n: usize| {
        message
            .sources
            .iter()
            .find(|s| s.n as usize == n && s.cited)
            .map(|s| (s.label.clone(), source_url(message.id, s)))
    };
    let page = |p: u32| {
        message
            .page_refs
            .iter()
            .find(|r| r.page == p)
            .map(|r| format!("/viewer/{}?page={p}&ref={}-{p}", r.document_id, message.id))
    };
    let html = markdown::render(
        &message.content,
        &Citations {
            source: &source,
            page: &page,
        },
    );
    let (cited, consulted): (Vec<&MessageSource>, Vec<&MessageSource>) =
        message.sources.iter().partition(|s| s.cited);
    let mut copy_text = markdown::plain(&message.content);
    if !cited.is_empty() {
        copy_text.push_str("\n\nFontes:\n");
        for s in &cited {
            copy_text.push_str(&format!("[{}] {}\n", s.n, s.label));
        }
    }
    AnswerView {
        id: message.id,
        status,
        html,
        error: message.error.clone().unwrap_or_default(),
        cited: cited.iter().map(|s| source_view(message.id, s)).collect(),
        consulted: consulted
            .iter()
            .map(|s| source_view(message.id, s))
            .collect(),
        copy_text: copy_text.trim_end().to_string(),
    }
}

fn turns(messages: &[Message]) -> Vec<TurnView> {
    let mut turns = Vec::new();
    let mut question: Option<String> = None;
    for m in messages {
        match m.role {
            Role::User => question = Some(m.content.clone()),
            Role::Assistant => turns.push(TurnView {
                question: question.take().unwrap_or_default(),
                answer: answer_view(m),
            }),
        }
    }
    turns
}

fn chat_service(state: &AppState) -> Result<&Arc<ChatService>, UiError> {
    state.chat.as_ref().map_err(|reason| UiError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        title: "Chat indisponível".into(),
        message: reason.clone(),
    })
}

fn ui_error(e: ChatError) -> UiError {
    match e {
        ChatError::NotFound => UiError::not_found(),
        ChatError::EmptyQuestion | ChatError::NotPending => UiError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            title: "Não foi possível enviar".into(),
            message: e.to_string(),
        },
        ChatError::Storage(m) => UiError::internal(m),
    }
}

fn fragment(template: impl Template) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => error_fragment(UiError::from(e)),
    }
}

fn error_fragment(e: UiError) -> Response {
    let html = format!(
        r#"<div class="alert alert-danger" role="alert"><div class="alert-content"><p class="alert-title">{}</p><p>{}</p></div></div>"#,
        markdown::escape(&e.title),
        markdown::escape(&e.message)
    );
    (e.status, Html(html)).into_response()
}

#[derive(Deserialize, Default)]
pub struct ChatQuery {
    /// Opens this document in the viewer.
    view: Option<DocumentId>,
}

async fn render_page(
    state: &AppState,
    conversation: Conversation,
    view: Option<DocumentId>,
) -> Result<String, UiError> {
    let chat = chat_service(state)?;
    let documents: Vec<DocumentOption> = match &state.ingestion {
        Ok(ingestion) => ingestion
            .list()
            .await
            .map_err(|e| UiError::internal(e.message))?
            .into_iter()
            .filter(|d| d.chunk_count > 0)
            .map(|d| DocumentOption {
                selected: conversation.document_id == Some(d.id),
                id: d.id,
                title: d.title,
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    let messages = chat.messages(conversation.id).await.map_err(ui_error)?;
    let recent: Vec<RecentView> = chat
        .recent(RECENT_CONVERSATIONS)
        .await
        .map_err(ui_error)?
        .into_iter()
        .filter(|c: &ConversationSummary| c.messages > 0 || c.id == conversation.id)
        .map(|c| RecentView {
            current: c.id == conversation.id,
            title: c.title.unwrap_or_else(|| "Nova conversa".into()),
            id: c.id,
        })
        .collect();
    let status = state.system_status.execute().await.language_model;
    let model_notice = (!status.is_available()).then(|| {
        let mut view = LanguageModelView::from(&status);
        view.detail = describe_model_status(&status);
        view
    });
    let scope_title = documents
        .iter()
        .find(|d| d.selected)
        .map_or_else(|| "Todos os documentos".to_string(), |d| d.title.clone());
    Ok(ChatPage {
        conversation_id: conversation.id,
        title: conversation
            .title
            .clone()
            .unwrap_or_else(|| "Nova conversa".into()),
        enabled: !documents.is_empty(),
        documents,
        scope_title,
        recent,
        turns: turns(&messages),
        suggestions: SUGGESTIONS,
        model_notice,
        viewer_html: match view {
            Some(document) => {
                crate::viewer::render(state, document, &crate::viewer::ViewerQuery::default())
                    .await?
            }
            None => String::new(),
        },
    }
    .render()?)
}

/// `GET /chat[?view={doc}]`: the most recent conversation (optionally with a document open).
pub async fn current(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ChatQuery>,
) -> Response {
    let content = async {
        let conversation = chat_service(&state)?.current().await.map_err(ui_error)?;
        render_page(&state, conversation, query.view).await
    }
    .await;
    page(&headers, Some(Section::Chat), content)
}

/// `GET /chat/{id}`.
pub async fn conversation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(query): Query<ChatQuery>,
) -> Response {
    let content = async {
        let conversation = chat_service(&state)?
            .conversation(id)
            .await
            .map_err(ui_error)?;
        render_page(&state, conversation, query.view).await
    }
    .await;
    page(&headers, Some(Section::Chat), content)
}

#[derive(Deserialize)]
pub struct NewForm {
    #[serde(default)]
    document: Option<String>,
}

fn parse_document(value: Option<&str>) -> Option<DocumentId> {
    value.and_then(|v| v.parse().ok())
}

/// `POST /chat/new`: a new conversation, keeping the chosen document.
pub async fn new(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<NewForm>,
) -> Response {
    let content = async {
        let conversation = chat_service(&state)?
            .start(parse_document(form.document.as_deref()))
            .await
            .map_err(ui_error)?;
        render_page(&state, conversation, None).await
    }
    .await;
    page(&headers, Some(Section::Chat), content)
}

/// `POST /chat/{id}/scope`: which document the conversation is about.
pub async fn scope(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<NewForm>,
) -> Response {
    let content = async {
        let chat = chat_service(&state)?;
        chat.set_scope(id, parse_document(form.document.as_deref()))
            .await
            .map_err(ui_error)?;
        render_page(&state, chat.conversation(id).await.map_err(ui_error)?, None).await
    }
    .await;
    page(&headers, Some(Section::Chat), content)
}

/// `POST /chat/{id}/delete`: removes the conversation and opens the most recent one.
pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Response {
    let content = async {
        let chat = chat_service(&state)?;
        chat.delete(id).await.map_err(ui_error)?;
        render_page(&state, chat.current().await.map_err(ui_error)?, None).await
    }
    .await;
    page(&headers, Some(Section::Chat), content)
}

#[derive(Deserialize)]
pub struct QuestionForm {
    q: String,
}

/// `POST /chat/{id}/messages`: saves the question; the answer streams afterwards.
pub async fn ask(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<QuestionForm>,
) -> Response {
    let result = async {
        let chat = chat_service(&state)?;
        let (_, assistant) = chat.ask(id, &form.q).await.map_err(ui_error)?;
        let answer = chat.message(assistant).await.map_err(ui_error)?;
        Ok::<_, UiError>(TurnFragment {
            turn: TurnView {
                question: form.q.trim().to_string(),
                answer: answer_view(&answer),
            },
        })
    }
    .await;
    match result {
        Ok(t) => fragment(t),
        Err(e) => error_fragment(e),
    }
}

/// `GET /chat/messages/{id}`: the answer as it is now (final once generated).
pub async fn answer(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    let result = async {
        let message = chat_service(&state)?.message(id).await.map_err(ui_error)?;
        Ok::<_, UiError>(AnswerFragment {
            answer: answer_view(&message),
        })
    }
    .await;
    match result {
        Ok(a) => fragment(a),
        Err(e) => error_fragment(e),
    }
}

/// `POST /chat/messages/{id}/regenerate`: back to the placeholder, which streams again.
pub async fn regenerate(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    let result = async {
        let chat = chat_service(&state)?;
        chat.regenerate(id).await.map_err(ui_error)?;
        let message = chat.message(id).await.map_err(ui_error)?;
        Ok::<_, UiError>(AnswerFragment {
            answer: answer_view(&message),
        })
    }
    .await;
    match result {
        Ok(a) => fragment(a),
        Err(e) => error_fragment(e),
    }
}
