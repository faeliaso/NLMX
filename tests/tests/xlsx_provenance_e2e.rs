//! A workbook keeps its structure through the real pipeline: parse → chunk → SQLite → embeddings
//! → retrieval → RAG. Every chunk knows its document, its sheet and its rows, carries the column
//! names of its records, and refers to no page and no viewer; embeddings and retrieval are the
//! code every other format uses.

mod support;

use std::sync::Arc;

use nlmx_application::{
    ports::{CancelFlag, DocumentRepository},
    services::{
        rag::{AnswerStatus, RagEngine, RagOptions},
        retrieval::HybridRetriever,
        retriever::{Retriever, RetrieverOptions},
    },
};
use nlmx_domain::{
    document_type::DocumentType, ingestion::DocumentId, parsed::DocumentChunk,
    retrieval::RetrievalFilter, source::SourceLocation,
};
use nlmx_testing::FakeLlmProvider;
use support::multiformat::{App, imported, office_fixture};

struct Workbook {
    app: App,
    id: DocumentId,
    chunks: Vec<DocumentChunk>,
}

impl Workbook {
    async fn new(name: &str, fixture: &str) -> Self {
        let app = App::new(name, true);
        let path = app.user_file(&office_fixture(fixture), fixture);
        let id = imported(app.ingestion.import(&path).await).0;
        let chunks = app.db.chunks_of(id).await.unwrap();
        Self { app, id, chunks }
    }

    fn retriever(&self) -> Arc<Retriever> {
        Arc::new(Retriever::new(Arc::new(HybridRetriever::new(
            self.app.model.clone(),
            self.app.db.clone(),
            self.app.db.clone(),
            self.app.db.clone(),
        ))))
    }

    async fn retrieve(
        &self,
        question: &str,
    ) -> Vec<nlmx_application::services::retriever::Passage> {
        self.retriever()
            .retrieve(
                question,
                &RetrieverOptions {
                    top_k: 10,
                    max_per_document: 10,
                    min_score: 0.0,
                    filter: RetrievalFilter {
                        documents: Some(vec![self.id]),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .passages
    }
}

fn xlsx_parts(chunk: &DocumentChunk) -> (u32, String, u32, u32) {
    match &chunk.location {
        SourceLocation::Xlsx {
            sheet_index,
            sheet_name,
            row_start,
            row_end,
        } => (*sheet_index, sheet_name.clone(), *row_start, *row_end),
        other => panic!("not an XLSX location: {other:?}"),
    }
}

#[tokio::test]
async fn one_worksheet_gives_chunks_with_its_sheet_rows_columns_and_metadata() {
    let wb = Workbook::new("xlsx-one", "uma-aba.xlsx").await;
    assert_eq!(wb.chunks.len(), 1);
    let chunk = &wb.chunks[0];
    assert_eq!(chunk.document_id, wb.id);
    assert_eq!(xlsx_parts(chunk), (1, "Janeiro".into(), 2, 2));
    assert_eq!(chunk.section_path, ["Janeiro"]);
    assert_eq!(chunk.metadata.columns, ["Produto", "Quantidade", "Valor"]);
    assert_eq!(chunk.metadata.document_type, DocumentType::Xlsx);
    assert_eq!(chunk.metadata.file_name.as_deref(), Some("uma-aba.xlsx"));
    // Columns travel with the values, never bare values.
    assert!(
        chunk
            .text
            .contains("Planilha: Janeiro\nColunas: Produto, Quantidade, Valor\nLinha 2\n")
    );
    assert!(
        chunk
            .text
            .contains("Produto: Notebook\nQuantidade: 10\nValor: 5000")
    );
    assert_eq!(chunk.location.page_range(), None);
    assert!(chunk.location.boxes().is_empty());
    assert!(!chunk.location.previewable());
    assert_eq!(chunk.location.label(), "Janeiro, linha 2");
}

#[tokio::test]
async fn several_worksheets_stay_apart_in_the_stored_chunks() {
    let wb = Workbook::new("xlsx-sheets", "varias-abas.xlsx").await;
    let sheets: Vec<_> = wb.chunks.iter().map(xlsx_parts).collect();
    assert_eq!(
        sheets,
        [
            (1, "Janeiro".into(), 2, 2),
            (2, "Fevereiro".into(), 2, 2),
            (3, "Março".into(), 2, 2),
        ]
    );
    for chunk in &wb.chunks {
        assert_eq!(chunk.section_path.len(), 1);
        let (_, name, ..) = xlsx_parts(chunk);
        assert_eq!(chunk.section_path[0], name);
        assert!(chunk.text.contains(&format!("Planilha: {name}\n")));
    }
    assert_eq!(
        wb.chunks.iter().map(|c| c.index).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    let sql = wb.app.sql();
    let stored: i64 = sql
        .query_row(
            "SELECT COUNT(*) FROM chunk_provenance WHERE json_extract(locator, '$.kind') = 'xlsx'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, 3);
    let with_page: i64 = sql
        .query_row(
            "SELECT COUNT(*) FROM chunk_provenance WHERE locator LIKE '%page%' OR locator LIKE '%box%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(with_page, 0, "no page and no coordinates");
}

#[tokio::test]
async fn a_large_sheet_makes_contiguous_chunks_and_a_row_is_found_by_its_value() {
    let wb = Workbook::new("xlsx-large", "muitas-linhas.xlsx").await;
    assert!(wb.chunks.len() > 1, "{} chunks", wb.chunks.len());
    let mut next = 2;
    for chunk in &wb.chunks {
        let (sheet, name, start, end) = xlsx_parts(chunk);
        assert_eq!((sheet, name.as_str()), (1, "Itens"));
        assert_eq!(start, next, "no gap and no overlap");
        next = end + 1;
        assert_eq!(chunk.metadata.columns, ["Id", "Item"]);
        assert!(chunk.text.contains("Colunas: Id, Item"));
    }
    assert_eq!(next, 302, "the 300 data rows are all covered");

    // Embeddings are the usual ones: one vector per chunk.
    let embedded: i64 = wb.app.count("SELECT COUNT(*) FROM chunk_embeddings");
    assert_eq!(embedded as usize, wb.chunks.len());

    // Retrieval finds the row; its source says sheet and rows, and no page.
    let passages = wb.retrieve("Item número 250").await;
    let hit = passages
        .iter()
        .find(|p| p.content.contains("Item: Item número 250"))
        .expect("the chunk with that row is retrieved");
    let source = &hit.provenance;
    assert_eq!(source.document_type(), DocumentType::Xlsx);
    assert_eq!(source.document_name, "muitas-linhas.xlsx");
    assert!(!source.previewable());
    assert_eq!(source.location().page_range(), None);
    let SourceLocation::Xlsx {
        sheet_name,
        row_start,
        row_end,
        ..
    } = source.location()
    else {
        panic!("an XLSX location");
    };
    assert_eq!(sheet_name, "Itens");
    assert!(
        *row_start <= 251 && 251 <= *row_end,
        "{row_start}–{row_end}"
    );
    assert_eq!(
        source.label(),
        format!("muitas-linhas.xlsx · Itens, linhas {row_start}–{row_end}")
    );
}

#[tokio::test]
async fn mixed_data_is_retrieved_and_cited_without_a_viewer() {
    let wb = Workbook::new("xlsx-mixed", "vendas.xlsx").await;
    let passages = wb.retrieve("Cadeira Nordeste 1250.5").await;
    let hit = passages
        .iter()
        .find(|p| p.content.contains("Produto: Cadeira"))
        .expect("the Resumo row is retrieved");
    assert_eq!(hit.provenance.label(), "vendas.xlsx · Resumo, linhas 2–4");
    assert!(hit.content.contains("Vendas: 1250.5"));
    assert!(
        hit.content.contains("Região (2): 2024-03-01"),
        "a date reads as a date: {}",
        hit.content
    );

    let llm = Arc::new(FakeLlmProvider::available().answering("Veja [1][2][3][4][5]."));
    let answer = RagEngine::new(wb.retriever(), wb.app.db.clone(), llm.clone())
        .ask(
            "Quanto vendeu a Cadeira no Nordeste?",
            &RagOptions {
                retriever: RetrieverOptions {
                    top_k: 10,
                    max_per_document: 5,
                    min_score: 0.0,
                    ..Default::default()
                },
                min_relevance: 0.0,
                ..Default::default()
            },
            &|_| {},
            CancelFlag::default(),
        )
        .await
        .unwrap();
    assert_eq!(answer.status, AnswerStatus::Answered);
    assert!(!answer.citations.is_empty());
    for citation in &answer.citations {
        assert_eq!(citation.provenance.document_type(), DocumentType::Xlsx);
        assert!(citation.viewer_target().is_none());
    }
    let prompt = &llm.requests()[0].user;
    assert!(
        prompt.contains(r#"tipo="xlsx" localizacao="Resumo, linhas 2–4""#),
        "{prompt}"
    );
    assert!(!prompt.contains("paginas="), "{prompt}");
}

#[tokio::test]
async fn rows_around_an_excel_table_keep_their_place_and_the_table_keeps_its_columns() {
    let wb = Workbook::new("xlsx-table", "tabela-excel.xlsx").await;
    assert_eq!(wb.chunks.len(), 1);
    let chunk = &wb.chunks[0];
    assert_eq!(xlsx_parts(chunk), (1, "Relatório".into(), 1, 8));
    assert_eq!(chunk.metadata.columns, ["Produto", "Região", "Total"]);
    let text = &chunk.text;
    let title = text.find("Linha 1: Relatório de vendas 2024").unwrap();
    let first = text.find("Produto: Notebook").unwrap();
    let note = text.find("Linha 8: Valores em reais").unwrap();
    assert!(title < first && first < note, "reading order: {text}");
}
