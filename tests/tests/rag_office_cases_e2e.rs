//! DOCX and XLSX are first-class sources of the existing RAG: seven retrieval cases over a library
//! of every format (PDF, Markdown, TXT, CSV, EPUB, DOCX, XLSX) through the real pipeline and the
//! hybrid retriever. For each case the model must be given enough content to answer, every source
//! keeps its provenance and its label, and only a PDF has an action that opens a viewer.

mod support;

use std::sync::Arc;

use nlmx_application::{
    ports::{CancelFlag, ConversationRepository},
    services::{
        free_chat::FreeChat,
        rag::{AnswerStatus, RagAnswer, RagEngine, RagOptions},
        retrieval::HybridRetriever,
        retriever::{Retriever, RetrieverOptions},
    },
    use_cases::ChatService,
};
use nlmx_domain::{chat::ConversationScope, document_type::DocumentType};
use nlmx_testing::FakeLlmProvider;
use support::{
    fixture,
    multiformat::{App, epub_fixture, imported},
    root,
};

/// What a case needs from one source: a passage of the answer that must reach the model, and how
/// the source is labelled for the user.
struct Need {
    /// Words of the content that answer the question; the prompt must contain them.
    content: &'static str,
    /// The start of the label: `<file> · <where>`.
    label: &'static str,
}

struct Case {
    kind: &'static str,
    question: &'static str,
    needs: Vec<Need>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            kind: "only-docx",
            question: "Qual a expiração do token JWT emitido pelo serviço de autenticação do backend?",
            needs: vec![Need {
                content: "tokens JWT com expiração de quinze minutos",
                label: "Manual.docx · Arquitetura › Backend",
            }],
        },
        Case {
            kind: "only-xlsx",
            question: "Qual a meta de satisfação do cliente no painel de indicadores?",
            needs: vec![Need {
                content: "Indicador: Satisfação do cliente",
                label: "Indicadores.xlsx · Dashboard, linhas ",
            }],
        },
        Case {
            kind: "pdf+docx",
            question: "Quais exames laboratoriais estão cobertos e em quanto tempo o resultado chega ao prontuário?",
            needs: vec![
                Need {
                    content: "Exames laboratoriais",
                    label: "Arquitetura.pdf · p. 1",
                },
                Need {
                    content: "resultado dos exames laboratoriais ao prontuário em até quatro horas",
                    label: "Manual.docx · Arquitetura › Backend",
                },
            ],
        },
        Case {
            kind: "pdf+xlsx",
            question: "Quem resolve os casos omissos e qual a meta de dias úteis para a operadora?",
            needs: vec![
                Need {
                    content: "Casos omissos serão resolvidos pela operadora",
                    label: "Arquitetura.pdf · p. 3",
                },
                Need {
                    content: "Meta: Casos omissos resolvidos pela operadora",
                    label: "Indicadores.xlsx · Metas, linhas 2–4",
                },
            ],
        },
        Case {
            kind: "markdown+docx",
            question: "Como o certificado do painel expirado é renovado?",
            needs: vec![
                Need {
                    content: "O código E-9020 indica que o certificado do painel expirou",
                    label: "manual.md · Manual de Operação › Erros conhecidos, linhas ",
                },
                Need {
                    content: "A renovação do certificado do painel é automatizada pelo pipeline de deploy",
                    label: "Manual.docx · Deploy",
                },
            ],
        },
        Case {
            kind: "csv+xlsx",
            question: "Quantas unidades de Cafe Torrado foram vendidas e qual a meta por trimestre?",
            needs: vec![
                Need {
                    content: "Cafe Torrado",
                    label: "vendas.csv · linha",
                },
                Need {
                    content: "Meta: Unidades vendidas de Cafe Torrado",
                    label: "Indicadores.xlsx · Metas, linhas 2–4",
                },
            ],
        },
        Case {
            kind: "many-formats",
            question: "Quem é o fornecedor Aurora e o que ele entrega?",
            needs: vec![
                Need {
                    content: "O fornecedor Aurora entrega as baterias dos nobreaks",
                    label: "manual.md · Manual de Operação › Compras",
                },
                Need {
                    content: "adotar o fornecedor Aurora",
                    label: "reuniao.txt · caracteres ",
                },
                Need {
                    content: "O fornecedor Aurora também entrega o hardware",
                    label: "Manual.docx · Deploy",
                },
                Need {
                    content: "Meta: Entregas do fornecedor Aurora no prazo",
                    label: "Indicadores.xlsx · Metas, linhas 2–4",
                },
            ],
        },
    ]
}

/// The library: one file of each format, named as a user would.
async fn library(name: &str) -> App {
    let app = App::new(name, true);
    for (display, source) in [
        ("Arquitetura.pdf", fixture("report.pdf")),
        ("manual.md", root().join("tests/golden/corpus/manual.md")),
        (
            "reuniao.txt",
            root().join("tests/golden/corpus/reuniao.txt"),
        ),
        ("vendas.csv", root().join("tests/golden/corpus/vendas.csv")),
        ("livro.epub", epub_fixture("livro.epub")),
        (
            "Manual.docx",
            root().join("tests/golden/corpus/operacoes.docx"),
        ),
        (
            "Indicadores.xlsx",
            root().join("tests/golden/corpus/indicadores.xlsx"),
        ),
    ] {
        let path = app.user_file(&source, display);
        imported(app.ingestion.import(&path).await);
    }
    app
}

fn rag(app: &App, llm: Arc<FakeLlmProvider>) -> RagEngine {
    let retriever = Arc::new(Retriever::new(Arc::new(HybridRetriever::new(
        app.model.clone(),
        app.db.clone(),
        app.db.clone(),
        app.db.clone(),
    ))));
    RagEngine::new(retriever, app.db.clone(), llm)
}

/// What the app asks for (6 passages, at most 4 per document), with the relevance floors off: the
/// deterministic embedder used here is not semantic, so a floor would only drop true positives.
fn options() -> RagOptions {
    RagOptions {
        retriever: RetrieverOptions {
            min_score: 0.0,
            ..Default::default()
        },
        min_relevance: 0.0,
        ..Default::default()
    }
}

async fn ask(app: &App, question: &str) -> (RagAnswer, String) {
    let llm = Arc::new(FakeLlmProvider::available().answering("Veja [1][2][3][4][5][6]."));
    let answer = rag(app, llm.clone())
        .ask(question, &options(), &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    let prompt = llm.requests()[0].user.clone();
    (answer, prompt)
}

#[tokio::test]
async fn the_model_gets_enough_content_and_every_source_keeps_its_provenance() {
    let app = library("rag-office-cases").await;
    for case in cases() {
        let (answer, prompt) = ask(&app, case.question).await;
        assert_eq!(answer.status, AnswerStatus::Answered, "{}", case.kind);

        for need in &case.needs {
            // The model is given the content that answers.
            assert!(
                prompt.contains(need.content),
                "{}: the prompt lacks {:?}\n{prompt}",
                case.kind,
                need.content
            );
            // The source is there, with its label.
            let source = answer
                .sources
                .iter()
                .find(|s| s.provenance.label().starts_with(need.label))
                .unwrap_or_else(|| {
                    let labels: Vec<_> = answer
                        .sources
                        .iter()
                        .map(|s| s.provenance.label())
                        .collect();
                    panic!(
                        "{}: no source labelled {:?} in {labels:?}",
                        case.kind, need.label
                    )
                });
            let kind = source.provenance.document_type();
            // The RAG gives the location in the vocabulary of the format and nothing to open.
            assert_eq!(
                source.provenance.previewable(),
                kind == DocumentType::Pdf,
                "{}",
                case.kind
            );
            match kind {
                DocumentType::Docx => assert!(
                    prompt.contains(&format!(
                        r#"tipo="docx" localizacao="{}""#,
                        source.provenance.location().label()
                    )),
                    "{}: {prompt}",
                    case.kind
                ),
                DocumentType::Xlsx => assert!(
                    prompt.contains(&format!(
                        r#"tipo="xlsx" localizacao="{}""#,
                        source.provenance.location().label()
                    )),
                    "{}: {prompt}",
                    case.kind
                ),
                _ => {}
            }
        }

        // Citations keep the provenance of the passage they cite, and only a PDF can be opened.
        assert!(answer.grounded, "{}", case.kind);
        for citation in &answer.citations {
            let source = answer.sources.iter().find(|s| s.n == citation.n).unwrap();
            assert_eq!(citation.provenance, source.provenance, "{}", case.kind);
            assert_eq!(
                citation.viewer_target().is_some(),
                citation.provenance.document_type() == DocumentType::Pdf,
                "{}: {}",
                case.kind,
                citation.provenance.label()
            );
        }
        // The labels read like the examples of the product.
        for source in &answer.sources {
            let label = source.provenance.label();
            match source.provenance.document_type() {
                DocumentType::Docx => assert!(label.starts_with("Manual.docx · "), "{label}"),
                DocumentType::Xlsx => {
                    assert!(label.starts_with("Indicadores.xlsx · "), "{label}");
                    assert!(label.contains("linha"), "{label}");
                }
                _ => {}
            }
            assert!(!label.contains("parágrafo"), "{label}");
        }
    }
}

#[tokio::test]
async fn only_the_docx_or_only_the_xlsx_answers_when_only_that_one_has_the_fact() {
    let app = library("rag-office-only").await;
    for case in cases().into_iter().filter(|c| c.kind.starts_with("only-")) {
        let (answer, _) = ask(&app, case.question).await;
        let first = &answer.sources[0].provenance;
        let expected = if case.kind == "only-docx" {
            DocumentType::Docx
        } else {
            DocumentType::Xlsx
        };
        assert_eq!(
            first.document_type(),
            expected,
            "{}: {}",
            case.kind,
            first.label()
        );
    }
}

#[tokio::test]
async fn a_conversation_over_every_format_stores_and_rereads_the_same_sources() {
    let app = library("rag-office-chat").await;
    let case = cases().pop().expect("the many-formats case");
    let llm = Arc::new(FakeLlmProvider::available().answering("Veja [1][2][3][4][5][6]."));
    let chat = ChatService {
        conversations: app.db.clone(),
        rag: Arc::new(rag(&app, llm.clone())),
        free: Arc::new(FreeChat::new(llm)),
        options: options(),
    };
    let c = chat.start(ConversationScope::Library).await.unwrap();
    let (_, answer) = chat.ask(c.id, case.question).await.unwrap();
    let message = chat
        .answer(answer, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    let saved = app.db.message(message.id).await.unwrap().expect("stored");
    assert_eq!(saved.sources.len(), message.sources.len());
    let mut kinds = std::collections::HashSet::new();
    for (before, after) in message.sources.iter().zip(&saved.sources) {
        assert_eq!(after.document_type(), before.document_type());
        assert_eq!(after.document_name, before.document_name);
        assert_eq!(
            after.reference.location, before.reference.location,
            "{}",
            before.label
        );
        assert_eq!(after.label, before.label);
        assert_eq!(
            after.previewable(),
            before.document_type() == DocumentType::Pdf
        );
        kinds.insert(after.document_type());
    }
    for kind in [
        DocumentType::Markdown,
        DocumentType::Text,
        DocumentType::Docx,
        DocumentType::Xlsx,
    ] {
        assert!(kinds.contains(&kind), "{kind:?} in {kinds:?}");
    }
}
