//! The RAG treats every format as an equivalent source of knowledge: PDF, Markdown, TXT, CSV and
//! EPUB go through the real indexing pipeline into SQLite (FTS5 + vec0), are retrieved, ranked,
//! put in the prompt and cited — and each result keeps the provenance of its chunk (document,
//! name, type, chunk, relevance, location, metadata). How a source is presented is not the
//! RAG's business: it only provides provenance.

mod support;

use std::{collections::HashSet, sync::Arc};

use nlmx_application::{
    ports::{CancelFlag, ChunkReader, ConversationRepository},
    services::{
        free_chat::FreeChat,
        rag::{AnswerStatus, RagEngine, RagOptions},
        retrieval::{HybridRetriever, RetrievalMode},
        retriever::{Passage, Retriever, RetrieverOptions},
    },
    use_cases::ChatService,
};
use nlmx_domain::{
    chat::ConversationScope, document_type::DocumentType, ingestion::DocumentId,
    retrieval::RetrievalFilter, source::SourceLocation,
};
use nlmx_testing::FakeLlmProvider;
use support::{
    fixture,
    multiformat::{App, epub_fixture, imported},
};

const MARKDOWN: &str = "# Arquitetura\n\n## Embeddings\n\n### Normalização\n\n\
A arquitetura utiliza uma camada de normalização antes da geração dos embeddings. \
A carência de dados ruins é tratada nessa camada.\n\n\
## Armazenamento\n\nO armazenamento usa SQLite com índices lexicais e vetoriais. \
A carência de memória é monitorada o tempo todo.\n";
const TEXT: &str = "Reunião sobre a camada de normalização.\n\n\
A carência de testes foi discutida: a normalização precisa de cobertura.\n";
const CSV: &str = "componente;etapa;descricao\n\
normalizacao;pre-embedding;camada de normalização do texto e carência zero\n\
indexacao;pos-embedding;gravação em SQLite\n";

/// One document of each format, indexed (and embedded unless `with_model` is false).
struct Corpus {
    app: App,
    pdf: DocumentId,
    markdown: DocumentId,
    text: DocumentId,
    csv: DocumentId,
    epub: DocumentId,
}

impl Corpus {
    async fn new(name: &str, with_model: bool) -> Self {
        let app = App::new(name, with_model);
        let inbox = app.dir.join("inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        let write = |file: &str, content: &str| {
            let path = inbox.join(file);
            std::fs::write(&path, content).unwrap();
            path
        };
        let files = [
            app.user_file(&fixture("report.pdf"), "report.pdf"),
            write("arquitetura.md", MARKDOWN),
            write("notas.txt", TEXT),
            write("dados.csv", CSV),
            app.user_file(&epub_fixture("livro.epub"), "livro.epub"),
        ];
        let mut ids = Vec::new();
        for path in &files {
            ids.push(imported(app.ingestion.import(path).await).0);
        }
        Self {
            pdf: ids[0],
            markdown: ids[1],
            text: ids[2],
            csv: ids[3],
            epub: ids[4],
            app,
        }
    }

    fn retriever(&self) -> Arc<Retriever> {
        Arc::new(Retriever::new(Arc::new(HybridRetriever::new(
            self.app.model.clone(),
            self.app.db.clone(),
            self.app.db.clone(),
            self.app.db.clone(),
        ))))
    }

    /// Passages for `question`, optionally restricted to one document.
    async fn retrieve(
        &self,
        question: &str,
        only: Option<DocumentId>,
    ) -> nlmx_application::services::retriever::RetrievalContext {
        self.retriever()
            .retrieve(
                question,
                &RetrieverOptions {
                    top_k: 30,
                    max_per_document: 10,
                    min_score: 0.0,
                    filter: RetrievalFilter {
                        documents: only.map(|d| vec![d]),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .unwrap()
    }

    fn rag(&self, llm: Arc<FakeLlmProvider>) -> RagEngine {
        RagEngine::new(self.retriever(), self.app.db.clone(), llm)
    }

    fn name_of(&self, id: DocumentId) -> &'static str {
        match id {
            i if i == self.pdf => "report.pdf",
            i if i == self.markdown => "arquitetura.md",
            i if i == self.text => "notas.txt",
            i if i == self.csv => "dados.csv",
            i if i == self.epub => "livro.epub",
            _ => panic!("unknown document {id}"),
        }
    }
}

fn rag_options() -> RagOptions {
    RagOptions {
        retriever: RetrieverOptions {
            top_k: 30,
            max_per_document: 10,
            min_score: 0.0,
            ..Default::default()
        },
        min_relevance: 0.0,
        ..Default::default()
    }
}

fn kinds(passages: &[Passage]) -> HashSet<DocumentType> {
    passages.iter().map(Passage::document_type).collect()
}

#[tokio::test]
async fn every_format_is_retrieved_with_its_own_provenance() {
    let corpus = Corpus::new("provenance", true).await;
    let cases = [
        (corpus.pdf, DocumentType::Pdf, "carência", "p"),
        (
            corpus.markdown,
            DocumentType::Markdown,
            "carência",
            "Armazenamento",
        ),
        (corpus.text, DocumentType::Text, "carência", "caracteres"),
        (corpus.csv, DocumentType::Csv, "carência", "linha"),
        (
            corpus.epub,
            DocumentType::Epub,
            "viagem de inverno",
            "cap. 1",
        ),
    ];
    for (id, kind, question, label_part) in cases {
        let name = corpus.name_of(id);
        let ctx = corpus.retrieve(question, Some(id)).await;
        assert!(!ctx.passages.is_empty(), "{name}: nothing retrieved");
        let p = &ctx.passages[0];
        let source = &p.provenance;

        assert_eq!(source.document_id(), id, "{name}");
        assert_eq!(source.document_type(), kind, "{name}");
        assert_eq!(source.document_name, name, "{name}");
        assert_eq!(source.reference.chunk_id, Some(p.chunk_id), "{name}");
        assert_eq!(source.relevance_score, p.score, "{name}");
        assert!(source.relevance_score > 0.0, "{name}");
        assert_eq!(source.metadata.document_type, kind, "{name}");
        // The location is the one of the format, with no PDF page invented for the others.
        let right_variant = matches!(
            (source.location(), kind),
            (SourceLocation::Pdf { .. }, DocumentType::Pdf)
                | (SourceLocation::Markdown { .. }, DocumentType::Markdown)
                | (SourceLocation::Text { .. }, DocumentType::Text)
                | (SourceLocation::Csv { .. }, DocumentType::Csv)
                | (SourceLocation::Epub { .. }, DocumentType::Epub)
        );
        assert!(right_variant, "{name}: {:?}", source.location());
        assert_eq!(source.previewable(), kind == DocumentType::Pdf, "{name}");
        // Label: "<file> · <where>".
        let label = source.label();
        assert!(label.starts_with(&format!("{name} · ")), "{label}");
        assert!(label.contains(label_part), "{name}: {label}");
        assert_eq!(p.source.label, label, "the passage shows the same label");
    }
}

#[tokio::test]
async fn the_labels_read_like_the_citations_the_user_sees() {
    let corpus = Corpus::new("labels", true).await;
    let label_of = |ctx: &nlmx_application::services::retriever::RetrievalContext, text: &str| {
        ctx.passages
            .iter()
            .find(|p| p.content.contains(text))
            .unwrap_or_else(|| panic!("no passage with {text:?}"))
            .provenance
            .label()
    };
    let md = corpus
        .retrieve("camada de normalização", Some(corpus.markdown))
        .await;
    assert_eq!(
        label_of(&md, "camada de normalização"),
        format!(
            "arquitetura.md · Arquitetura › Embeddings › Normalização, {}",
            lines_of(&md, "camada de normalização")
        )
    );
    let csv = corpus.retrieve("normalização", Some(corpus.csv)).await;
    assert_eq!(label_of(&csv, "normalização"), "dados.csv · linhas 1–2");
    let epub = corpus
        .retrieve("viagem de inverno", Some(corpus.epub))
        .await;
    assert!(label_of(&epub, "inverno").starts_with("livro.epub · cap. 1"));
}

/// "linhas A–B" of the passage containing `text` (the exact lines depend on the chunker).
fn lines_of(ctx: &nlmx_application::services::retriever::RetrievalContext, text: &str) -> String {
    let p = ctx
        .passages
        .iter()
        .find(|p| p.content.contains(text))
        .unwrap();
    match p.location() {
        SourceLocation::Markdown {
            line_start: Some(a),
            line_end: Some(b),
            ..
        } if a == b => format!("linha {a}"),
        SourceLocation::Markdown {
            line_start: Some(a),
            line_end: Some(b),
            ..
        } => format!("linhas {a}–{b}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn one_document_can_give_several_sources_in_different_places() {
    let corpus = Corpus::new("several", true).await;
    let ctx = corpus.retrieve("carência", Some(corpus.markdown)).await;
    let from_markdown: Vec<&Passage> = ctx
        .passages
        .iter()
        .filter(|p| p.provenance.document_id() == corpus.markdown)
        .collect();
    assert!(from_markdown.len() >= 2, "{from_markdown:#?}");
    let paths: HashSet<Vec<String>> = from_markdown
        .iter()
        .map(|p| match p.location() {
            SourceLocation::Markdown { heading_path, .. } => heading_path.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert!(
        paths.len() >= 2,
        "each source says its own section: {paths:?}"
    );
    let chunks: HashSet<_> = from_markdown.iter().map(|p| p.chunk_id).collect();
    assert_eq!(chunks.len(), from_markdown.len(), "distinct chunks");
}

#[tokio::test]
async fn several_formats_answer_together_and_all_are_cited() {
    let corpus = Corpus::new("mixed", true).await;
    let llm = Arc::new(
        FakeLlmProvider::available()
            .answering("A carência aparece nos documentos [1][2][3][4][5][6][7][8][9]."),
    );
    let answer = corpus
        .rag(llm.clone())
        .ask(
            "Onde se fala de carência?",
            &rag_options(),
            &|_| {},
            CancelFlag::default(),
        )
        .await
        .unwrap();
    assert_eq!(answer.status, AnswerStatus::Answered);

    let types: HashSet<DocumentType> = answer
        .sources
        .iter()
        .map(|s| s.provenance.document_type())
        .collect();
    for kind in [
        DocumentType::Pdf,
        DocumentType::Markdown,
        DocumentType::Text,
        DocumentType::Csv,
    ] {
        assert!(types.contains(&kind), "{kind:?} in the context: {types:?}");
    }
    // Every citation keeps the provenance of its source.
    assert!(answer.grounded);
    for citation in &answer.citations {
        let source = answer.sources.iter().find(|s| s.n == citation.n).unwrap();
        assert_eq!(citation.provenance, source.provenance);
        assert_eq!(
            citation.viewer_target().is_some(),
            citation.provenance.document_type() == DocumentType::Pdf,
            "only a PDF can be opened"
        );
    }
    let cited: HashSet<DocumentType> = answer
        .citations
        .iter()
        .map(|c| c.provenance.document_type())
        .collect();
    assert!(cited.len() >= 4, "{cited:?}");

    // The model is told where each passage is, by the vocabulary of its format.
    let prompt = &llm.requests()[0].user;
    assert!(
        prompt.contains(r#"tipo="markdown" localizacao=""#),
        "{prompt}"
    );
    assert!(
        prompt.contains(r#"tipo="text" localizacao="caracteres "#),
        "{prompt}"
    );
    assert!(
        prompt.contains(r#"tipo="csv" localizacao="linha"#),
        "{prompt}"
    );
    assert!(prompt.contains(r#"paginas=""#), "the PDF keeps its pages");
    assert!(
        !prompt.contains(r#"tipo="pdf""#),
        "the PDF header is the one it always had"
    );
    // Document text never reaches the instructions.
    assert!(!llm.requests()[0].system.contains("carência de dados ruins"));

    // A page reference never resolves to a source that has no pages.
    let pages_ok = answer.documents.iter().all(|d| {
        d.pages.is_empty()
            || answer.sources.iter().any(|s| {
                s.document_id == d.document_id && s.provenance.document_type() == DocumentType::Pdf
            })
    });
    assert!(pages_ok, "{:?}", answer.documents);
}

#[tokio::test]
async fn ranking_and_neighbour_joins_never_lose_provenance() {
    let corpus = Corpus::new("ranking", true).await;
    let ctx = corpus.retrieve("carência normalização", None).await;
    assert!(
        kinds(&ctx.passages).len() >= 3,
        "{:?}",
        kinds(&ctx.passages)
    );

    // Best first.
    assert!(ctx.passages.windows(2).all(|w| w[0].score >= w[1].score));
    let stored = corpus.app.db.clone();
    for p in &ctx.passages {
        let name = corpus.name_of(p.document_id);
        let source = &p.provenance;
        assert_eq!(source.document_name, name);
        assert_eq!(source.document_id(), p.document_id);
        assert_eq!(source.relevance_score, p.score, "{name}");
        assert_eq!(source.reference.chunk_id, Some(p.chunk_id), "{name}");
        assert!(p.metadata.chunk_ids.contains(&p.chunk_id), "{name}");
        // What is stored is what comes back: the location of a single chunk is its own.
        if let [only] = p.metadata.chunk_ids.as_slice() {
            let chunk = stored.get_many(&[*only]).await.unwrap().remove(0);
            assert_eq!(&chunk.location, source.location(), "{name}");
            assert_eq!(chunk.document_type, source.document_type(), "{name}");
        }
    }
    // Passages grouped by document keep pointing at the same documents.
    for group in &ctx.documents {
        for i in &group.passages {
            assert_eq!(ctx.passages[*i].document_id, group.document_id);
        }
    }
}

#[tokio::test]
async fn without_an_embedding_model_search_is_lexical_and_still_has_provenance() {
    let corpus = Corpus::new("lexical", false).await;
    let ctx = corpus.retrieve("carência", None).await;
    assert_eq!(ctx.mode, RetrievalMode::LexicalOnly);
    assert!(
        kinds(&ctx.passages).len() >= 4,
        "{:?}",
        kinds(&ctx.passages)
    );
    assert!(
        ctx.passages[0].provenance.relevance_score > 0.0,
        "the best hit scores"
    );
    for p in &ctx.passages {
        assert_eq!(
            p.provenance.document_type(),
            p.provenance.location().document_type()
        );
        assert!(p.provenance.relevance_score >= 0.0);
        assert!(!p.provenance.document_name.is_empty());
        assert!(p.provenance.label().contains(" · "));
    }
}

#[tokio::test]
async fn saved_answers_keep_each_source_with_its_provenance() {
    let corpus = Corpus::new("saved", true).await;
    let llm = Arc::new(
        FakeLlmProvider::available()
            .answering("A carência aparece nos documentos [1][2][3][4][5][6][7][8][9]."),
    );
    let chat = ChatService {
        conversations: corpus.app.db.clone(),
        rag: Arc::new(corpus.rag(llm.clone())),
        free: Arc::new(FreeChat::new(llm)),
        options: rag_options(),
    };
    let c = chat.start(ConversationScope::Library).await.unwrap();
    let (_, answer) = chat.ask(c.id, "Onde se fala de carência?").await.unwrap();
    let message = chat
        .answer(answer, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    assert!(message.sources.len() >= 4);

    // Read back from SQLite.
    let saved = corpus
        .app
        .db
        .message(message.id)
        .await
        .unwrap()
        .expect("the answer is stored");
    assert_eq!(saved.sources.len(), message.sources.len());
    for (before, after) in message.sources.iter().zip(&saved.sources) {
        assert_eq!(after.document_type(), before.document_type());
        assert_eq!(after.document_name, before.document_name);
        assert_eq!(
            after.reference.location, before.reference.location,
            "{}",
            before.label
        );
        assert_eq!(after.reference.document_id, before.document_id);
        assert_eq!(
            after.previewable(),
            before.document_type() == DocumentType::Pdf
        );
        assert_eq!(after.document_name, corpus.name_of(after.document_id));
        assert!(!after.quote.is_empty());
    }
    let saved_types: HashSet<DocumentType> =
        saved.sources.iter().map(|s| s.document_type()).collect();
    assert!(saved_types.len() >= 4, "{saved_types:?}");
    // The PDF still has its page and boxes where the viewer reads them.
    let pdf = saved
        .sources
        .iter()
        .find(|s| s.document_type() == DocumentType::Pdf)
        .unwrap();
    assert!(pdf.page_start >= 1 && !pdf.bboxes.is_empty());
}

/// Provenance carries file names and locations: none of it (nor any text) may reach the logs
/// (captured at TRACE through the app's redacting layer) or the metrics.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provenance_of_every_format_stays_out_of_logs() {
    use nlmx_telemetry::{Format, LogLayer, MemorySink, MetricsLayer, MetricsRegistry};
    use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt};

    const CANARY: &str = "CANARIO-4b7d";
    let sink = MemorySink::default();
    let metrics = MetricsRegistry::new();
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry()
            .with(LogLayer::new(sink.clone(), Format::Json).with_filter(EnvFilter::new("trace")))
            .with(MetricsLayer::new(metrics.clone())),
    )
    .unwrap();

    let app = App::new("rag-canary", true);
    let inbox = app.dir.join("inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    for (file, content) in [
        (
            format!("{CANARY} guia.md"),
            format!("# {CANARY}\n\n## Seção {CANARY}\n\nO paciente {CANARY} tem carência.\n"),
        ),
        (
            format!("{CANARY} notas.txt"),
            format!("Prontuário {CANARY}\n\nA carência de {CANARY} é de trinta dias.\n"),
        ),
        (
            format!("{CANARY} dados.csv"),
            format!("nome;carencia\n{CANARY};30\noutro;60\n"),
        ),
    ] {
        let path = inbox.join(file);
        std::fs::write(&path, content).unwrap();
        imported(app.ingestion.import(&path).await);
    }
    let pdf = app.user_file(&fixture("canary.pdf"), &format!("{CANARY} laudo.pdf"));
    imported(app.ingestion.import(&pdf).await);

    let corpus_retriever = Arc::new(Retriever::new(Arc::new(HybridRetriever::new(
        app.model.clone(),
        app.db.clone(),
        app.db.clone(),
        app.db.clone(),
    ))));
    let llm = Arc::new(
        FakeLlmProvider::available()
            .answering(&format!("A carência de {CANARY} é de 30 dias [1][2][3]")),
    );
    let chat = ChatService {
        conversations: app.db.clone(),
        rag: Arc::new(RagEngine::new(
            corpus_retriever,
            app.db.clone(),
            llm.clone(),
        )),
        free: Arc::new(FreeChat::new(llm)),
        options: rag_options(),
    };
    let c = chat.start(ConversationScope::Library).await.unwrap();
    let (_, answer) = chat
        .ask(c.id, &format!("Qual a carência de {CANARY}?"))
        .await
        .unwrap();
    let message = chat
        .answer(answer, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    assert!(!message.sources.is_empty());
    assert!(
        message
            .sources
            .iter()
            .any(|s| s.document_name.contains(CANARY)),
        "the file name is in the provenance (it is data, not log)"
    );

    let lines = sink.0.lock().unwrap();
    assert!(!lines.is_empty(), "logs were captured");
    let leaks: Vec<&String> = lines.iter().filter(|l| l.contains(CANARY)).collect();
    assert!(leaks.is_empty(), "canary in logs: {leaks:#?}");
    assert!(
        !nlmx_telemetry::snapshot_json(&metrics.snapshot())
            .to_string()
            .contains(CANARY)
    );
}
