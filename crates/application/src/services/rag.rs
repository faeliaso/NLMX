//! RAG Engine: question → Retriever → relevance gate → ContextBuilder → LlmProvider →
//! CitationEngine. Answers only from the retrieved passages, with `[n]` citations.

pub mod citations;
pub mod context;

use std::{sync::Arc, time::Instant};

use nlmx_domain::{
    generation::{FinishReason, GenerationRequest, LanguageModelStatus, LlmError},
    ingestion::DocumentId,
    rag_intent::{QueryIntent, section_matches},
    retrieval::normalize_query,
    telemetry::{ErrorKind, Measurement},
};

use self::{
    citations::{Citation, CitationEngine, DocumentRef, PageRef, PageReference},
    context::{
        BuiltContext, ContextBudget, ContextBuilder, NOT_FOUND_ANSWER, OVERVIEW_TASK, SECTION_TASK,
        Source, neutralize,
    },
};
use super::{
    retrieval::{RetrievalError, RetrievalMode},
    retriever::{Passage, Retriever, RetrieverOptions, structural_passage},
};
use nlmx_domain::vectors::ChunkId;

use crate::{
    ports::{CancelFlag, ChunkReader, ChunkView, LlmProvider},
    telemetry::{ms, record},
};

pub const REFUSED_ANSWER: &str =
    "O modelo não pôde responder a esta pergunta. Veja os trechos encontrados abaixo.";

#[derive(Debug, Clone, PartialEq)]
pub struct RagOptions {
    pub retriever: RetrieverOptions,
    /// Below this best passage score (0–1) the answer is "not found" without calling the model.
    pub min_relevance: f32,
    pub max_context_tokens: u32,
    pub answer_tokens: u32,
    pub max_passage_tokens: u32,
    pub temperature: f32,
}

impl Default for RagOptions {
    fn default() -> Self {
        let budget = ContextBudget::default();
        Self {
            retriever: RetrieverOptions::default(),
            min_relevance: 0.35,
            max_context_tokens: budget.max_context_tokens,
            answer_tokens: budget.answer_tokens,
            max_passage_tokens: budget.max_passage_tokens,
            temperature: 0.2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerStatus {
    Answered,
    /// Nothing relevant was retrieved, or the model found no answer in the passages.
    NotFound,
    /// The model's guardrails declined to answer.
    Refused,
    /// Stopped by the user; `answer` holds what was generated.
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RagAnswer {
    pub question: String,
    pub answer: String,
    pub status: AnswerStatus,
    /// Passages sent to the model (or, without a generation, the best ones retrieved).
    pub sources: Vec<Source>,
    pub citations: Vec<Citation>,
    /// Cited documents and pages.
    pub documents: Vec<DocumentRef>,
    pub pages: Vec<PageRef>,
    /// `[página N]` references in the answer.
    pub page_refs: Vec<PageReference>,
    /// The answer cites at least one passage.
    pub grounded: bool,
    pub mode: RetrievalMode,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RagError {
    Retrieval(RetrievalError),
    /// The language model can't be used; `sources` are the passages found anyway.
    ModelUnavailable {
        status: LanguageModelStatus,
        sources: Vec<Source>,
    },
    Generation(LlmError),
}

impl std::fmt::Display for RagError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Retrieval(e) => e.fmt(f),
            Self::ModelUnavailable { status, .. } => match status {
                LanguageModelStatus::LicenseRequired => LlmError::LicenseRequired.fmt(f),
                LanguageModelStatus::NotInstalled => {
                    f.write_str("O Apple Foundation Models não está instalado neste Mac.")
                }
                LanguageModelStatus::Incompatible { reason }
                | LanguageModelStatus::Unavailable { reason, .. } => {
                    LlmError::Unavailable(reason.clone()).fmt(f)
                }
                LanguageModelStatus::Available => f.write_str("modelo de linguagem indisponível"),
            },
            Self::Generation(e) => e.fmt(f),
        }
    }
}

/// How a failed answer is recorded.
pub(crate) fn failure_kind(e: &RagError) -> ErrorKind {
    match e {
        RagError::Retrieval(_) => ErrorKind::Storage,
        RagError::ModelUnavailable { .. } => ErrorKind::Unavailable,
        RagError::Generation(LlmError::Timeout) => ErrorKind::Timeout,
        RagError::Generation(LlmError::Refused(_)) => ErrorKind::Refused,
        RagError::Generation(LlmError::Unavailable(_) | LlmError::LicenseRequired) => {
            ErrorKind::Unavailable
        }
        RagError::Generation(LlmError::ContextTooLong { .. }) => ErrorKind::Invalid,
        RagError::Generation(LlmError::Protocol(_)) => ErrorKind::Other,
    }
}

/// Search results added to a section's passages.
const SECTION_EXTRA_PASSAGES: usize = 3;

/// Exact-count retries: each one drops the lowest-scoring passage.
const MAX_FIT_ATTEMPTS: usize = 4;

pub const CHOOSE_DOCUMENT_ANSWER: &str =
    "Escolha um documento no seletor acima para que eu possa explicá-lo.";

/// A previous exchange of the conversation (for follow-up questions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryTurn {
    pub question: String,
    pub answer: String,
}

const REWRITE_INSTRUCTIONS: &str = concat!(
    "\
Reescreva a última pergunta do usuário como uma pergunta completa e independente, que possa ser \
entendida sem a conversa. Responda somente com a pergunta reescrita. Se ela já \
for independente, repita-a.
",
    nlmx_domain::response_language_auto!()
);
#[cfg(test)]
pub(crate) const REWRITE_INSTRUCTIONS_FOR_TESTS: &str = REWRITE_INSTRUCTIONS;
/// Previous turns considered when rewriting a follow-up question.
const REWRITE_TURNS: usize = 3;

/// Passages chosen for a question, before the prompt is built.
struct Selection {
    question: String,
    passages: Vec<Passage>,
    mode: RetrievalMode,
    warnings: Vec<String>,
    task: Option<&'static str>,
    /// Passage size cap for this selection (overviews spread the budget over sections).
    max_passage_tokens: Option<u32>,
}

/// Answer without calling the model.
struct Early {
    text: String,
    passages: Vec<Passage>,
    mode: RetrievalMode,
    warnings: Vec<String>,
}

/// The passages to answer from, or an answer that needs no generation.
type Selected = Result<Selection, Early>;

pub struct RagEngine {
    retriever: Arc<Retriever>,
    chunks: Arc<dyn ChunkReader>,
    llm: Arc<dyn LlmProvider>,
}

impl RagEngine {
    pub fn new(
        retriever: Arc<Retriever>,
        chunks: Arc<dyn ChunkReader>,
        llm: Arc<dyn LlmProvider>,
    ) -> Self {
        Self {
            retriever,
            chunks,
            llm,
        }
    }

    pub async fn ask(
        &self,
        question: &str,
        options: &RagOptions,
        on_token: &(dyn Fn(&str) + Send + Sync),
        cancel: CancelFlag,
    ) -> Result<RagAnswer, RagError> {
        self.converse(question, &[], options, on_token, cancel)
            .await
    }

    /// Answers `question` in a conversation: a follow-up is first rewritten as a standalone
    /// question; "explain this document" and "section N" questions use the document structure.
    pub async fn converse(
        &self,
        question: &str,
        history: &[HistoryTurn],
        options: &RagOptions,
        on_token: &(dyn Fn(&str) + Send + Sync),
        cancel: CancelFlag,
    ) -> Result<RagAnswer, RagError> {
        let question = normalize_query(question);
        let single_document = options
            .retriever
            .filter
            .documents
            .as_ref()
            .is_some_and(|d| d.len() == 1);
        let mut intent = if single_document {
            QueryIntent::parse_in_document(&question)
        } else {
            QueryIntent::parse(&question)
        };
        let mut standalone = question.clone();
        if intent == QueryIntent::Regular && !history.is_empty() {
            standalone = self.rewrite(&question, history).await;
            intent = QueryIntent::parse(&standalone);
        }
        let budget = ContextBudget {
            context_tokens: self.llm.capabilities().context_tokens,
            max_context_tokens: options.max_context_tokens,
            answer_tokens: options.answer_tokens,
            max_passage_tokens: options.max_passage_tokens,
            ..ContextBudget::default()
        };

        let selection = match intent {
            QueryIntent::Regular => {
                let found = self.search(&standalone, options).await?;
                // A rewrite that drifted must not turn an answerable question into "not found".
                if found.is_err() && standalone != question {
                    standalone = question.clone();
                    self.search(&standalone, options).await?
                } else {
                    found
                }
            }
            QueryIntent::Overview => self.overview(&standalone, options, &budget).await?,
            QueryIntent::Section(label) => self.section(&standalone, &label, options).await?,
        };
        let selection = match selection {
            Ok(selection) => selection,
            Err(early) => {
                let builder = ContextBuilder::new(budget, options.temperature);
                let sources = builder.build(&standalone, &early.passages).sources;
                return Ok(RagAnswer {
                    question,
                    answer: early.text,
                    status: AnswerStatus::NotFound,
                    sources,
                    citations: Vec::new(),
                    documents: Vec::new(),
                    pages: Vec::new(),
                    page_refs: Vec::new(),
                    grounded: false,
                    mode: early.mode,
                    warnings: early.warnings,
                });
            }
        };
        let mut builder = ContextBuilder::new(
            ContextBudget {
                max_passage_tokens: selection
                    .max_passage_tokens
                    .unwrap_or(budget.max_passage_tokens),
                ..budget
            },
            options.temperature,
        );
        builder.task = selection.task;
        self.generate(question, selection, &builder, on_token, cancel)
            .await
    }

    /// Regular question: hybrid search + relevance gate.
    async fn search(&self, question: &str, options: &RagOptions) -> Result<Selected, RagError> {
        let context = self
            .retriever
            .retrieve(question, &options.retriever)
            .await
            .map_err(RagError::Retrieval)?;
        // Relevance gate: weak retrieval never reaches the model.
        let best = context.passages.first().map_or(0.0, |p| p.score);
        if best < options.min_relevance {
            return Ok(Err(Early {
                text: NOT_FOUND_ANSWER.into(),
                passages: context.passages,
                mode: context.mode,
                warnings: context.warnings,
            }));
        }
        Ok(Ok(Selection {
            question: context.question,
            passages: context.passages,
            mode: context.mode,
            warnings: context.warnings,
            task: None,
            max_passage_tokens: None,
        }))
    }

    /// Documents the question may draw from: the filter's, or every searchable one.
    async fn scope(&self, options: &RagOptions) -> Result<Vec<DocumentId>, RagError> {
        let filter = &options.retriever.filter;
        let unavailable = |e: crate::ports::StorageError| {
            RagError::Retrieval(RetrievalError::Unavailable(e.message))
        };
        if filter.documents.is_some() || filter.collections.is_some() {
            let candidates = self.chunks.resolve(filter).await.map_err(unavailable)?;
            return Ok(candidates.documents.unwrap_or_default());
        }
        self.chunks.chunked_documents().await.map_err(unavailable)
    }

    /// "Explique este documento.": the start of each section, in reading order.
    async fn overview(
        &self,
        question: &str,
        options: &RagOptions,
        budget: &ContextBudget,
    ) -> Result<Selected, RagError> {
        let documents = self.scope(options).await?;
        let [document] = documents.as_slice() else {
            return Ok(Err(Early {
                text: CHOOSE_DOCUMENT_ANSWER.into(),
                passages: Vec::new(),
                mode: RetrievalMode::Structure,
                warnings: Vec::new(),
            }));
        };
        let chunks = self.document_chunks(*document).await?;
        let mut passages: Vec<Passage> = Vec::new();
        let mut last_section: Option<Option<String>> = None;
        for chunk in chunks {
            if last_section.as_ref() == Some(&chunk.section) {
                continue;
            }
            last_section = Some(chunk.section.clone());
            passages.push(structural_passage(vec![chunk]));
        }
        // Spread the budget so that every section gets a share.
        let builder = ContextBuilder::new(*budget, options.temperature).with_task(OVERVIEW_TASK);
        let per_section =
            (builder.passage_budget(question) / passages.len().max(1) as u32).clamp(120, 600);
        Ok(Ok(Selection {
            question: question.to_string(),
            passages,
            mode: RetrievalMode::Structure,
            warnings: Vec::new(),
            task: Some(OVERVIEW_TASK),
            max_passage_tokens: Some(per_section),
        }))
    }

    /// "Quais são os principais pontos da seção 3?": the chunks of that section.
    async fn section(
        &self,
        question: &str,
        label: &str,
        options: &RagOptions,
    ) -> Result<Selected, RagError> {
        let mut passages = Vec::new();
        for document in self.scope(options).await? {
            let mut run: Vec<ChunkView> = Vec::new();
            for chunk in self.document_chunks(document).await? {
                let in_section = chunk
                    .section
                    .as_deref()
                    .is_some_and(|s| section_matches(s, label));
                let continues = run.last().is_some_and(|last| {
                    last.section == chunk.section
                        && last.ordinal + 1 == chunk.ordinal
                        && run.len() < 3
                });
                if (!in_section || !continues) && !run.is_empty() {
                    passages.push(structural_passage(std::mem::take(&mut run)));
                }
                if in_section {
                    run.push(chunk);
                }
            }
            if !run.is_empty() {
                passages.push(structural_passage(run));
            }
        }
        if passages.is_empty() {
            // No heading matches (e.g. "cláusula 7" written in the body, or headings the
            // structure analysis did not keep): answer from the search instead.
            return self.search(question, options).await;
        }
        // "Seção N" may also be how the user names a fact that lives elsewhere ("a carência da
        // cláusula 1"): the best search results join the section's passages.
        if let Ok(context) = self.retriever.retrieve(question, &options.retriever).await {
            let known: Vec<ChunkId> = passages
                .iter()
                .flat_map(|p: &Passage| p.metadata.chunk_ids.clone())
                .collect();
            passages.extend(
                context
                    .passages
                    .into_iter()
                    .filter(|p| p.score >= options.min_relevance)
                    .filter(|p| !p.metadata.chunk_ids.iter().any(|c| known.contains(c)))
                    .take(SECTION_EXTRA_PASSAGES),
            );
        }
        Ok(Ok(Selection {
            question: question.to_string(),
            passages,
            mode: RetrievalMode::Structure,
            warnings: Vec::new(),
            task: Some(SECTION_TASK),
            max_passage_tokens: None,
        }))
    }

    async fn document_chunks(&self, document: DocumentId) -> Result<Vec<ChunkView>, RagError> {
        self.chunks
            .document_chunks(document)
            .await
            .map_err(|e| RagError::Retrieval(RetrievalError::Unavailable(e.message)))
    }

    /// A follow-up ("e para consultas?") rewritten as a standalone question. Falls back to the
    /// original on any failure; history text is neutralized like document text.
    async fn rewrite(&self, question: &str, history: &[HistoryTurn]) -> String {
        if !self.llm.status().await.is_available() {
            return question.to_string();
        }
        let shorten = |text: &str| -> String {
            let text: String = text.chars().take(300).collect();
            neutralize(&text).replace('\n', " ")
        };
        let mut user = String::from("<conversa>\n");
        for turn in history.iter().rev().take(REWRITE_TURNS).rev() {
            user.push_str(&format!(
                "Usuário: {}\nAssistente: {}\n",
                shorten(&turn.question),
                shorten(&turn.answer)
            ));
        }
        user.push_str(&format!(
            "</conversa>\n\n<ultima_pergunta>\n{}\n</ultima_pergunta>",
            neutralize(question)
        ));
        let request = GenerationRequest {
            system: REWRITE_INSTRUCTIONS.into(),
            history: Vec::new(),
            user,
            temperature: 0.0,
            max_tokens: 120,
        };
        match self
            .llm
            .generate(&request, &|_| {}, CancelFlag::default())
            .await
        {
            Ok(g) if g.finish == FinishReason::Completed => {
                let rewritten = normalize_query(g.text.trim().trim_matches('"'));
                if rewritten.is_empty() || rewritten.chars().count() > 400 {
                    question.to_string()
                } else {
                    rewritten
                }
            }
            _ => question.to_string(),
        }
    }

    /// Generates and records the measurement (intent, prompt size, first token, total).
    async fn generate(
        &self,
        question: String,
        selection: Selection,
        builder: &ContextBuilder,
        on_token: &(dyn Fn(&str) + Send + Sync),
        cancel: CancelFlag,
    ) -> Result<RagAnswer, RagError> {
        let started = Instant::now();
        let first_token: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        let prompt_tokens = std::sync::atomic::AtomicU32::new(0);
        let timed = |text: &str| {
            first_token.get_or_init(|| ms(started));
            on_token(text);
        };
        let result = self
            .generate_inner(question, selection, builder, &timed, cancel, &prompt_tokens)
            .await;
        match &result {
            Ok(answer) => record(&Measurement::Generated {
                intent: match builder.task {
                    Some(OVERVIEW_TASK) => "overview",
                    Some(SECTION_TASK) => "section",
                    _ => "regular",
                },
                status: match answer.status {
                    AnswerStatus::Answered => "answered",
                    AnswerStatus::NotFound => "not_found",
                    AnswerStatus::Refused => "refused",
                    AnswerStatus::Cancelled => "cancelled",
                },
                prompt_tokens: prompt_tokens.load(std::sync::atomic::Ordering::Relaxed),
                first_token_ms: first_token.get().copied(),
                output_chars: answer.answer.chars().count() as u32,
                total_ms: ms(started),
            }),
            Err(e) => record(&Measurement::GenerationFailed {
                kind: failure_kind(e),
                total_ms: ms(started),
            }),
        }
        result
    }

    async fn generate_inner(
        &self,
        question: String,
        selection: Selection,
        builder: &ContextBuilder,
        on_token: &(dyn Fn(&str) + Send + Sync),
        cancel: CancelFlag,
        prompt_tokens: &std::sync::atomic::AtomicU32,
    ) -> Result<RagAnswer, RagError> {
        let Selection {
            question: prompt_question,
            passages,
            mode,
            mut warnings,
            ..
        } = selection;
        let answer = |text: &str, status, sources, warnings| RagAnswer {
            question: question.clone(),
            answer: text.to_string(),
            status,
            sources,
            citations: Vec::new(),
            documents: Vec::new(),
            pages: Vec::new(),
            page_refs: Vec::new(),
            grounded: false,
            mode,
            warnings,
        };

        let status = self.llm.status().await;
        if !status.is_available() {
            let sources = builder.build(&prompt_question, &passages).sources;
            return Err(RagError::ModelUnavailable { status, sources });
        }

        let (built, tokens) = self.fit(builder, &prompt_question, passages).await?;
        prompt_tokens.store(tokens, std::sync::atomic::Ordering::Relaxed);
        if built
            .dropped
            .iter()
            .any(|d| d.reason == context::DropReason::Budget)
        {
            warnings.push("alguns trechos ficaram de fora por limite de contexto".into());
        }
        if built.sources.iter().any(|s| s.truncated) {
            warnings.push("trechos longos foram encurtados".into());
        }

        let generation = match self.llm.generate(&built.request, on_token, cancel).await {
            Ok(g) => g,
            Err(LlmError::Refused(reason)) => {
                warnings.push(reason);
                return Ok(answer(
                    REFUSED_ANSWER,
                    AnswerStatus::Refused,
                    built.sources,
                    warnings,
                ));
            }
            Err(e) => return Err(RagError::Generation(e)),
        };
        let cited = CitationEngine::resolve(&generation.text, &built.sources);
        if !cited.invalid.is_empty() {
            let list: Vec<String> = cited.invalid.iter().map(|n| format!("[{n}]")).collect();
            warnings.push(format!(
                "citações sem trecho correspondente removidas: {}",
                list.join(", ")
            ));
        }
        let status = match generation.finish {
            FinishReason::Cancelled => AnswerStatus::Cancelled,
            _ if cited.not_found => AnswerStatus::NotFound,
            _ => AnswerStatus::Answered,
        };
        if generation.finish == FinishReason::Length {
            warnings.push("a resposta foi interrompida pelo limite de tamanho".into());
        }
        if !cited.unresolved_pages.is_empty() {
            warnings.push("referências de página sem documento correspondente".into());
        }
        if status == AnswerStatus::Answered && !cited.grounded() {
            warnings.push("a resposta não cita nenhum trecho".into());
        }
        Ok(RagAnswer {
            question,
            grounded: cited.grounded(),
            answer: cited.text,
            status,
            sources: built.sources,
            citations: cited.citations,
            documents: cited.documents,
            pages: cited.pages,
            page_refs: cited.page_refs,
            mode,
            warnings,
        })
    }

    /// Builds the context and confirms it with the model's exact token count, dropping the
    /// lowest-scoring passage until it fits.
    async fn fit(
        &self,
        builder: &ContextBuilder,
        question: &str,
        mut passages: Vec<Passage>,
    ) -> Result<(BuiltContext, u32), RagError> {
        let limit = builder
            .budget
            .context_tokens
            .saturating_sub(builder.budget.answer_tokens);
        let mut attempt = 0;
        loop {
            let built = builder.build(question, &passages);
            let tokens = self
                .llm
                .count_tokens(&built.request)
                .await
                .map_err(RagError::Generation)?;
            if tokens <= limit {
                return Ok((built, tokens));
            }
            attempt += 1;
            if built.sources.len() <= 1 || attempt > MAX_FIT_ATTEMPTS {
                return Err(RagError::Generation(LlmError::ContextTooLong {
                    tokens,
                    limit,
                }));
            }
            // Drop the weakest passage that made it into the prompt.
            let weakest = built
                .sources
                .iter()
                .min_by(|a, b| a.score.total_cmp(&b.score))
                .map(|s| s.chunk_id);
            passages.retain(|p| Some(p.chunk_id) != weakest);
        }
    }
}
