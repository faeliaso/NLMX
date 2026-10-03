//! RAG quality on a golden set (`tests/golden/rag.json`): retrieval hit@1, hit@5 and MRR for
//! answerable questions, and "not found" for questions the documents can't answer.
//! The deterministic run (hash embedder, lexical in practice) guards against regressions; the
//! `#[ignore]`d run uses Qwen3 embeddings and Apple Foundation Models (`make test-real`) and also
//! measures the citation validity rate.

mod support;

use std::sync::Arc;

use nlmx_application::{
    ports::{CancelFlag, EmbeddingSource, LlmProvider},
    services::{
        rag::{AnswerStatus, RagOptions},
        retriever::RetrieverOptions,
    },
};
use nlmx_testing::FakeLlmProvider;
use support::{Library, deterministic_embeddings, root, temp_dir};

struct Golden {
    corpus: Vec<String>,
    questions: Vec<(String, Vec<(String, u32)>)>,
}

fn golden() -> Golden {
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("tests/golden/rag.json")).unwrap(),
    )
    .unwrap();
    Golden {
        corpus: v["corpus"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f.as_str().unwrap().into())
            .collect(),
        questions: v["questions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| {
                (
                    q["question"].as_str().unwrap().into(),
                    q["expect"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|e| (e[0].as_str().unwrap().into(), e[1].as_u64().unwrap() as u32))
                        .collect(),
                )
            })
            .collect(),
    }
}

#[derive(Debug, Default)]
struct Quality {
    answerable: usize,
    hit1: usize,
    hit5: usize,
    reciprocal: f64,
    unanswerable: usize,
    not_found_correct: usize,
    cited: usize,
    citations_valid: usize,
}

impl Quality {
    fn hit_at_1(&self) -> f64 {
        self.hit1 as f64 / self.answerable as f64
    }
    fn hit_at_5(&self) -> f64 {
        self.hit5 as f64 / self.answerable as f64
    }
    fn mrr(&self) -> f64 {
        self.reciprocal / self.answerable as f64
    }
    fn not_found_accuracy(&self) -> f64 {
        self.not_found_correct as f64 / self.unanswerable.max(1) as f64
    }
    fn citation_validity(&self) -> Option<f64> {
        (self.cited > 0).then(|| self.citations_valid as f64 / self.cited as f64)
    }
}

async fn evaluate(
    name: &str,
    embeddings: Arc<dyn EmbeddingSource>,
    llm: Arc<dyn LlmProvider>,
) -> Quality {
    let golden = golden();
    let mut library = Library::new(temp_dir(name), embeddings);
    let files: Vec<&str> = golden.corpus.iter().map(String::as_str).collect();
    library.import(&files).await;
    let retriever = library.retriever();
    let rag = library.rag(llm);
    let options = RetrieverOptions {
        min_score: 0.0,
        ..Default::default()
    };
    let mut q = Quality::default();
    for (question, expected) in &golden.questions {
        let matches = |doc: i64, start: u32, end: u32| {
            expected
                .iter()
                .any(|(file, page)| library.file_of(doc) == file && (start..=end).contains(page))
        };
        if expected.is_empty() {
            q.unanswerable += 1;
            let answer = rag
                .ask(
                    question,
                    &RagOptions::default(),
                    &|_| {},
                    CancelFlag::default(),
                )
                .await
                .unwrap();
            if answer.status == AnswerStatus::NotFound {
                q.not_found_correct += 1;
            } else {
                eprintln!("✗ not-found expected: {question} → {:?}", answer.status);
            }
            continue;
        }
        q.answerable += 1;
        let ctx = retriever.retrieve(question, &options).await.unwrap();
        let rank = ctx
            .passages
            .iter()
            .position(|p| matches(p.document_id, p.source.page_start, p.source.page_end));
        match rank {
            Some(r) => {
                q.hit1 += usize::from(r == 0);
                q.hit5 += usize::from(r < 5);
                q.reciprocal += 1.0 / (r + 1) as f64;
            }
            None => eprintln!("✗ not retrieved: {question}"),
        }
        let answer = rag
            .ask(
                question,
                &RagOptions::default(),
                &|_| {},
                CancelFlag::default(),
            )
            .await
            .unwrap();
        if !answer.citations.is_empty() {
            q.cited += 1;
            if answer
                .citations
                .iter()
                .any(|c| matches(c.document_id, c.page_start, c.page_end))
            {
                q.citations_valid += 1;
            }
        }
    }
    eprintln!(
        "[{name}] hit@1 {:.2} · hit@5 {:.2} · MRR {:.2} · not-found {:.2} · citation validity {:?}",
        q.hit_at_1(),
        q.hit_at_5(),
        q.mrr(),
        q.not_found_accuracy(),
        q.citation_validity()
    );
    q
}

#[tokio::test]
async fn golden_set_with_the_deterministic_embedder() {
    // The scripted model cites [1]: citation validity then measures whether the best source is right.
    let llm = Arc::new(FakeLlmProvider::available().answering("Resposta [1]."));
    let q = evaluate("rag-quality", deterministic_embeddings(), llm).await;
    assert!(q.hit_at_5() >= 0.85, "{q:?}");
    assert!(q.hit_at_1() >= 0.6, "{q:?}");
    assert!(q.mrr() >= 0.7, "{q:?}");
    assert_eq!(q.not_found_accuracy(), 1.0, "{q:?}");
}

#[tokio::test]
#[ignore = "real Qwen3 embeddings + Apple Foundation Models (make test-real)"]
async fn golden_set_with_real_models() {
    use nlmx_embed_llama::{EmbeddingConfig, llama_server_path, provider};
    let dir = temp_dir("rag-quality-real");
    let config = EmbeddingConfig::load(&root().join("models/embedding.example.json"))
        .expect("scripts/fetch-embedding-model.sh");
    let llama = Arc::new(provider(
        config,
        llama_server_path().expect("make bootstrap"),
        &dir,
    ));
    let embeddings = nlmx_testing::FixedEmbeddingSource::of_arc(llama.clone());
    let fm = Arc::new(nlmx_llm_fm::FoundationModelsProvider::system(
        dir.join("run"),
    ));
    let q = evaluate("rag-quality-real", embeddings, fm.clone()).await;
    fm.shutdown().await;
    llama.shutdown().await;
    assert!(q.hit_at_5() >= 0.9, "{q:?}");
    assert!(q.mrr() >= 0.75, "{q:?}");
    assert_eq!(q.not_found_accuracy(), 1.0, "{q:?}");
    assert!(q.citation_validity().unwrap_or(0.0) >= 0.8, "{q:?}");
}
