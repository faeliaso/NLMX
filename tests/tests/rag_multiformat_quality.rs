//! RAG quality over a corpus with one document of each format (`tests/golden/rag_multiformat.json`):
//! questions whose answer is only in the PDF, only in the Markdown, only in the TXT, only in the
//! CSV, only in the EPUB, spread over two formats and over several, and questions the documents
//! cannot answer. A source counts only if it is the right document **and** the right place
//! (page, heading, text range, rows, chapter): provenance is part of the answer.
//!
//! The deterministic run (hash embedder, lexical in practice) guards against regressions; the
//! `#[ignore]`d run uses Qwen3 embeddings and Apple Foundation Models (`make test-real`).

mod support;

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use nlmx_application::{
    ports::{CancelFlag, LlmProvider},
    services::{
        free_chat::FreeChat,
        rag::{AnswerStatus, RagEngine, RagOptions},
        retrieval::HybridRetriever,
        retriever::{Passage, Retriever, RetrieverOptions},
    },
};
use nlmx_domain::{ingestion::DocumentId, source::SourceLocation};
use nlmx_testing::FakeLlmProvider;
use support::{
    multiformat::{App, imported},
    root,
};

#[derive(Debug, Clone)]
struct Expected {
    file: String,
    check: Check,
}

/// Where in the file the answer is.
#[derive(Debug, Clone)]
enum Check {
    Pages(Vec<u32>),
    Heading(String),
    /// A phrase of a TXT (its range must contain it) or of a CSV (its rows must contain it).
    Phrase(String),
    Chapter(u32),
}

struct Question {
    kind: String,
    text: String,
    expect: Vec<Expected>,
}

struct Golden {
    corpus: Vec<(String, PathBuf)>,
    questions: Vec<Question>,
}

fn golden() -> Golden {
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("tests/golden/rag_multiformat.json")).unwrap(),
    )
    .unwrap();
    let text = |x: &serde_json::Value| x.as_str().unwrap().to_string();
    Golden {
        corpus: v["corpus"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (text(&f["file"]), root().join(text(&f["source"]))))
            .collect(),
        questions: v["questions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| Question {
                kind: text(&q["kind"]),
                text: text(&q["question"]),
                expect: q["expect"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| Expected {
                        file: text(&e["file"]),
                        check: if let Some(p) = e.get("pages") {
                            Check::Pages(
                                p.as_array()
                                    .unwrap()
                                    .iter()
                                    .map(|n| n.as_u64().unwrap() as u32)
                                    .collect(),
                            )
                        } else if let Some(h) = e.get("heading") {
                            Check::Heading(text(h))
                        } else if let Some(p) = e.get("phrase") {
                            Check::Phrase(text(p))
                        } else {
                            Check::Chapter(e["chapter"].as_u64().unwrap() as u32)
                        },
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// The 1-based data rows (header not counted) of a CSV whose line contains `phrase`.
fn csv_rows_with(source: &str, phrase: &str) -> Vec<u32> {
    source
        .lines()
        .skip(1)
        .enumerate()
        .filter(|(_, line)| line.contains(phrase))
        .map(|(i, _)| i as u32 + 1)
        .collect()
}

struct Corpus {
    app: App,
    ids: BTreeMap<String, DocumentId>,
    /// The original text of the TXT and CSV files, to check ranges and rows against.
    sources: BTreeMap<String, String>,
}

impl Corpus {
    async fn new(app: App, golden: &Golden) -> Self {
        let mut ids = BTreeMap::new();
        let mut sources = BTreeMap::new();
        for (file, source) in &golden.corpus {
            let path = app.user_file(source, file);
            ids.insert(file.clone(), imported(app.ingestion.import(&path).await).0);
            if file.ends_with(".txt") || file.ends_with(".csv") {
                sources.insert(file.clone(), std::fs::read_to_string(source).unwrap());
            }
        }
        Self { app, ids, sources }
    }

    fn name_of(&self, id: DocumentId) -> &str {
        self.ids
            .iter()
            .find(|(_, v)| **v == id)
            .map(|(k, _)| k.as_str())
            .unwrap()
    }

    /// Whether `passage` is the right document and the right place for `expected`.
    fn place_matches(&self, passage: &Passage, expected: &Expected) -> bool {
        if self.name_of(passage.document_id) != expected.file {
            return false;
        }
        match (&expected.check, passage.location()) {
            (
                Check::Pages(pages),
                SourceLocation::Pdf {
                    page_start,
                    page_end,
                    ..
                },
            ) => pages.iter().any(|p| (*page_start..=*page_end).contains(p)),
            (Check::Heading(h), SourceLocation::Markdown { heading_path, .. }) => {
                heading_path.iter().any(|x| x == h)
            }
            (Check::Phrase(phrase), SourceLocation::Text { start, end }) => {
                // Offsets are characters of the decoded text, not bytes.
                let text = &self.sources[&expected.file];
                let range: String = text
                    .chars()
                    .skip(*start as usize)
                    .take((*end - *start) as usize)
                    .collect();
                range.contains(phrase.as_str()) && passage.content.contains(phrase.as_str())
            }
            (Check::Phrase(phrase), SourceLocation::Csv { row_start, row_end }) => {
                csv_rows_with(&self.sources[&expected.file], phrase)
                    .iter()
                    .any(|r| (*row_start..=*row_end).contains(r))
                    && passage.content.contains(phrase.as_str())
            }
            (Check::Chapter(n), SourceLocation::Epub { chapter_index, .. }) => chapter_index == n,
            _ => false,
        }
    }
}

#[derive(Debug, Default)]
struct Quality {
    /// Expected sources: found in the first 5 passages (right document), and with the right place.
    expected: usize,
    found_in_5: usize,
    placed_in_5: usize,
    first_rank_sum: f64,
    questions: usize,
    questions_complete: usize,
    unanswerable: usize,
    not_found_correct: usize,
    cited: usize,
    cited_right: usize,
    by_kind: BTreeMap<String, (usize, usize)>,
}

impl Quality {
    fn found(&self) -> f64 {
        self.found_in_5 as f64 / self.expected.max(1) as f64
    }
    fn placed(&self) -> f64 {
        self.placed_in_5 as f64 / self.expected.max(1) as f64
    }
}

/// Builds the library (the model is installed first, so every chunk is embedded with it), asks
/// every question and measures retrieval, provenance, "not found" and citations.
async fn evaluate(name: &str, install: impl FnOnce(&App), llm: Arc<dyn LlmProvider>) -> Quality {
    let golden = golden();
    let app = App::new(name, false);
    install(&app);
    let corpus = Corpus::new(app, &golden).await;
    let app = &corpus.app;
    let retriever = Arc::new(Retriever::new(Arc::new(HybridRetriever::new(
        app.model.clone(),
        app.db.clone(),
        app.db.clone(),
        app.db.clone(),
    ))));
    let rag = RagEngine::new(retriever.clone(), app.db.clone(), llm.clone());
    let _free = FreeChat::new(llm);
    // What the app retrieves (6 passages, at most 4 per document); the deterministic embedder is
    // not semantic, so the relevance floor is off.
    let options = RetrieverOptions {
        min_score: 0.0,
        ..Default::default()
    };
    let mut q = Quality::default();
    for question in &golden.questions {
        if question.expect.is_empty() {
            q.unanswerable += 1;
            let answer = rag
                .ask(
                    &question.text,
                    &RagOptions::default(),
                    &|_| {},
                    CancelFlag::default(),
                )
                .await
                .unwrap();
            if answer.status == AnswerStatus::NotFound {
                q.not_found_correct += 1;
            } else {
                eprintln!(
                    "✗ not-found expected: {} → {:?}",
                    question.text, answer.status
                );
            }
            continue;
        }
        q.questions += 1;
        let ctx = retriever.retrieve(&question.text, &options).await.unwrap();
        if std::env::var_os("NLMX_DEBUG_RANK").is_some() {
            eprintln!("# {}", question.text);
            for (i, p) in ctx.passages.iter().enumerate() {
                eprintln!("  {:>2}. {:.3} {}", i + 1, p.score, p.provenance.label());
            }
        }
        let mut complete = true;
        for expected in &question.expect {
            q.expected += 1;
            let entry = q.by_kind.entry(question.kind.clone()).or_default();
            entry.0 += 1;
            let document = ctx
                .passages
                .iter()
                .position(|p| corpus.name_of(p.document_id) == expected.file);
            let placed = ctx
                .passages
                .iter()
                .position(|p| corpus.place_matches(p, expected));
            // "In the context" = among the passages the model would be given.
            q.found_in_5 += usize::from(document.is_some());
            match placed {
                Some(r) => {
                    q.placed_in_5 += 1;
                    entry.1 += 1;
                    q.first_rank_sum += 1.0 / (r + 1) as f64;
                }
                None => {
                    complete = false;
                    eprintln!(
                        "✗ {}: {:?} not retrieved with the right place ({})",
                        question.kind, expected, question.text
                    );
                }
            }
        }
        q.questions_complete += usize::from(complete);

        // The answer cites sources: each citation that points at an expected place is right.
        let answer = rag
            .ask(
                &question.text,
                &RagOptions::default(),
                &|_| {},
                CancelFlag::default(),
            )
            .await
            .unwrap();
        for citation in &answer.citations {
            q.cited += 1;
            let source = answer.sources.iter().find(|s| s.n == citation.n).unwrap();
            let hit = ctx.passages.iter().any(|p| {
                p.chunk_id == source.chunk_id
                    && question.expect.iter().any(|e| corpus.place_matches(p, e))
            });
            q.cited_right += usize::from(hit);
        }
    }
    eprintln!(
        "[{name}] sources in the context {:.2} · right place in the context {:.2} · questions complete {}/{} · \
         not-found {}/{} · citations right {}/{}",
        q.found(),
        q.placed(),
        q.questions_complete,
        q.questions,
        q.not_found_correct,
        q.unanswerable,
        q.cited_right,
        q.cited
    );
    for (kind, (total, placed)) in &q.by_kind {
        eprintln!("[{name}]   {kind}: {placed}/{total} with the right place in the context");
    }
    q
}

#[tokio::test]
async fn multiformat_golden_set_with_the_deterministic_embedder() {
    // The scripted model cites [1]: citation precision then measures whether the best source is right.
    let llm = Arc::new(FakeLlmProvider::available().answering("Resposta [1]."));
    let q = evaluate("rag-multiformat", |app| app.model.install(), llm).await;
    // Every expected source of every kind of question comes back with its right place.
    assert_eq!(q.placed(), 1.0, "{q:?}");
    assert_eq!(q.found(), 1.0, "{q:?}");
    assert_eq!(q.questions_complete, q.questions, "{q:?}");
    assert_eq!(q.not_found_correct, q.unanswerable, "{q:?}");
    for (kind, (total, placed)) in &q.by_kind {
        assert_eq!(total, placed, "{kind}");
    }
    assert!(q.cited > 0);
}

#[tokio::test]
#[ignore = "real Qwen3 embeddings + Apple Foundation Models (make test-real)"]
async fn multiformat_golden_set_with_real_models() {
    use nlmx_embed_llama::{EmbeddingConfig, llama_server_path, provider};
    let dir = support::temp_dir("rag-multiformat-real");
    let config = EmbeddingConfig::load(&root().join("models/embedding.example.json"))
        .expect("scripts/fetch-embedding-model.sh");
    let llama = Arc::new(provider(
        config,
        llama_server_path().expect("make bootstrap"),
        &dir,
    ));
    let fm = Arc::new(nlmx_llm_fm::FoundationModelsProvider::system(
        dir.join("run"),
    ));
    let q = {
        let llama = llama.clone();
        evaluate(
            "rag-multiformat-real",
            move |app| app.model.install_provider(llama),
            fm.clone(),
        )
        .await
    };
    fm.shutdown().await;
    llama.shutdown().await;
    assert!(q.placed() >= 0.85, "{q:?}");
    assert!(q.found() >= 0.9, "{q:?}");
    assert!(q.not_found_correct == q.unanswerable, "{q:?}");
    if q.cited > 0 {
        assert!(q.cited_right as f64 / q.cited as f64 >= 0.7, "{q:?}");
    }
}
