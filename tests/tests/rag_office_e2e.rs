//! DOCX and XLSX go through the real indexing pipeline into SQLite, are retrieved, used by the
//! RAG and cited with their own provenance — and, having no viewer, never get a viewer target.

mod support;

use std::sync::Arc;

use nlmx_application::{
    ports::{CancelFlag, ConversationRepository},
    services::{
        free_chat::FreeChat,
        rag::{AnswerStatus, RagEngine, RagOptions},
        retrieval::HybridRetriever,
        retriever::{Retriever, RetrieverOptions},
    },
    use_cases::ChatService,
};
use nlmx_domain::{
    chat::ConversationScope, document_type::DocumentType, ingestion::DocumentId,
    retrieval::RetrievalFilter, source::SourceLocation,
};
use nlmx_testing::FakeLlmProvider;
use support::multiformat::{App, imported, office_fixture};

struct Office {
    app: App,
    docx: DocumentId,
    xlsx: DocumentId,
}

impl Office {
    async fn new(name: &str) -> Self {
        let app = App::new(name, true);
        let docx = app.user_file(&office_fixture("contrato.docx"), "contrato.docx");
        let xlsx = app.user_file(&office_fixture("vendas.xlsx"), "vendas.xlsx");
        let docx = imported(app.ingestion.import(&docx).await).0;
        let xlsx = imported(app.ingestion.import(&xlsx).await).0;
        Self { app, docx, xlsx }
    }

    fn retriever(&self) -> Arc<Retriever> {
        Arc::new(Retriever::new(Arc::new(HybridRetriever::new(
            self.app.model.clone(),
            self.app.db.clone(),
            self.app.db.clone(),
            self.app.db.clone(),
        ))))
    }

    fn rag(&self, llm: Arc<FakeLlmProvider>) -> RagEngine {
        RagEngine::new(self.retriever(), self.app.db.clone(), llm)
    }
}

fn options() -> RagOptions {
    RagOptions {
        retriever: RetrieverOptions {
            top_k: 10,
            max_per_document: 5,
            min_score: 0.0,
            ..Default::default()
        },
        min_relevance: 0.0,
        ..Default::default()
    }
}

#[tokio::test]
async fn a_docx_and_an_xlsx_are_retrieved_with_their_own_provenance() {
    let office = Office::new("office-provenance").await;

    for (id, kind, question, name, label_part) in [
        (
            office.docx,
            DocumentType::Docx,
            "prazo de vigência",
            "contrato.docx",
            "Prazos",
        ),
        (
            office.xlsx,
            DocumentType::Xlsx,
            "Cadeira Nordeste",
            "vendas.xlsx",
            "Resumo",
        ),
    ] {
        let ctx = office
            .retriever()
            .retrieve(
                question,
                &RetrieverOptions {
                    top_k: 10,
                    max_per_document: 5,
                    min_score: 0.0,
                    filter: RetrievalFilter {
                        documents: Some(vec![id]),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(!ctx.passages.is_empty(), "{name}: nothing retrieved");
        let source = &ctx.passages[0].provenance;
        assert_eq!(source.document_id(), id);
        assert_eq!(source.document_type(), kind);
        assert_eq!(source.document_name, name);
        assert!(!source.previewable(), "{name}: no viewer");
        let right_variant = matches!(
            (source.location(), kind),
            (SourceLocation::Docx { .. }, DocumentType::Docx)
                | (SourceLocation::Xlsx { .. }, DocumentType::Xlsx)
        );
        assert!(right_variant, "{name}: {:?}", source.location());
        let label = source.label();
        assert!(label.starts_with(&format!("{name} · ")), "{label}");
        assert!(label.contains(label_part), "{name}: {label}");
    }
}

#[tokio::test]
async fn the_rag_answers_from_a_docx_and_an_xlsx_and_cites_them() {
    let office = Office::new("office-rag").await;
    let llm = Arc::new(FakeLlmProvider::available().answering("Veja [1][2][3][4][5][6][7][8][9]."));
    let answer = office
        .rag(llm)
        .ask(
            "Qual o prazo de vigência e quanto vendeu a Cadeira no Nordeste?",
            &options(),
            &|_| {},
            CancelFlag::default(),
        )
        .await
        .unwrap();
    assert_eq!(answer.status, AnswerStatus::Answered);
    let kinds: Vec<DocumentType> = answer
        .sources
        .iter()
        .map(|s| s.provenance.document_type())
        .collect();
    assert!(kinds.contains(&DocumentType::Docx), "{kinds:?}");
    assert!(kinds.contains(&DocumentType::Xlsx), "{kinds:?}");
    assert!(answer.grounded);
    let cited: Vec<DocumentType> = answer
        .citations
        .iter()
        .map(|c| c.provenance.document_type())
        .collect();
    assert!(cited.contains(&DocumentType::Docx), "{cited:?}");
    assert!(cited.contains(&DocumentType::Xlsx), "{cited:?}");
    for citation in &answer.citations {
        assert!(
            citation.viewer_target().is_none(),
            "a {} has no viewer to open",
            citation.provenance.document_type()
        );
    }
}

#[tokio::test]
async fn the_provenance_of_a_docx_and_an_xlsx_survives_the_database() {
    let office = Office::new("office-saved").await;
    let llm = Arc::new(FakeLlmProvider::available().answering("Veja [1][2][3][4][5][6][7][8][9]."));
    let chat = ChatService {
        conversations: office.app.db.clone(),
        rag: Arc::new(office.rag(llm.clone())),
        free: Arc::new(FreeChat::new(llm)),
        options: options(),
    };
    let c = chat.start(ConversationScope::Library).await.unwrap();
    let (_, answer) = chat
        .ask(c.id, "prazo de vigência e vendas de Cadeira")
        .await
        .unwrap();
    let message = chat
        .answer(answer, &|_| {}, CancelFlag::default())
        .await
        .unwrap();
    let saved = office
        .app
        .db
        .message(message.id)
        .await
        .unwrap()
        .expect("the answer is stored");
    assert_eq!(saved.sources.len(), message.sources.len());
    for (before, after) in message.sources.iter().zip(&saved.sources) {
        assert_eq!(after.document_type(), before.document_type());
        assert_eq!(after.reference.location, before.reference.location);
        assert!(!after.previewable());
        assert!(!after.quote.is_empty());
    }
    let kinds: Vec<DocumentType> = saved.sources.iter().map(|s| s.document_type()).collect();
    assert!(kinds.contains(&DocumentType::Docx) && kinds.contains(&DocumentType::Xlsx));
}

#[tokio::test]
async fn a_cell_of_a_docx_table_is_found_with_the_table_marker_and_no_viewer() {
    let office = Office::new("office-table").await;
    let ctx = office
        .retriever()
        .retrieve(
            "Parcela Final Valor",
            &RetrieverOptions {
                top_k: 10,
                max_per_document: 5,
                min_score: 0.0,
                filter: RetrievalFilter {
                    documents: Some(vec![office.docx]),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let hit = ctx
        .passages
        .iter()
        .find(|p| p.content.contains("Tabela 1: Parcela | Valor"))
        .expect("a passage built from the table");
    assert!(hit.content.contains("R$ 2.000") || hit.content.contains("R$ 1.000"));
    assert!(matches!(
        hit.provenance.location(),
        SourceLocation::Docx { .. }
    ));
    assert!(!hit.provenance.previewable());
    assert!(hit.provenance.label().starts_with("contrato.docx · "));
}

#[tokio::test]
async fn csv_and_xlsx_stay_different_formats_with_their_own_provenance() {
    let app = App::new("office-csv-vs-xlsx", true);
    let inbox = app.dir.join("inbox");
    std::fs::create_dir_all(&inbox).unwrap();
    let csv = inbox.join("dados.csv");
    std::fs::write(&csv, "produto;quantidade\nCadeira;10\nMesa;4\n").unwrap();
    let xlsx = app.user_file(&office_fixture("vendas.xlsx"), "vendas.xlsx");
    let csv_id = imported(app.ingestion.import(&csv).await).0;
    let xlsx_id = imported(app.ingestion.import(&xlsx).await).0;

    use nlmx_application::ports::DocumentRepository;
    let csv_chunks = app.db.chunks_of(csv_id).await.unwrap();
    let xlsx_chunks = app.db.chunks_of(xlsx_id).await.unwrap();
    assert!(
        csv_chunks
            .iter()
            .all(|c| matches!(c.location, SourceLocation::Csv { .. })
                && c.metadata.document_type == DocumentType::Csv)
    );
    assert!(
        xlsx_chunks
            .iter()
            .all(|c| matches!(c.location, SourceLocation::Xlsx { .. })
                && c.metadata.document_type == DocumentType::Xlsx)
    );
    // A CSV has no sheets and says rows; a workbook says its sheet.
    assert!(
        csv_chunks[0]
            .text
            .starts_with("Arquivo: dados.csv\nColunas:")
    );
    assert!(xlsx_chunks[0].text.contains("Planilha: Resumo"));
    assert_eq!(xlsx_chunks[0].location.label(), "Resumo, linhas 2–4");
    assert!(csv_chunks[0].location.label().starts_with("linha"));
    // One chunk per visible sheet here, numbered across sheets.
    let sheets: Vec<_> = xlsx_chunks
        .iter()
        .map(|c| c.section_path.join(""))
        .collect();
    assert_eq!(sheets, ["Resumo", "Notas"]);
    assert_eq!(
        xlsx_chunks.iter().map(|c| c.index).collect::<Vec<_>>(),
        [0, 1]
    );
}
