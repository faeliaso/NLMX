//! `MultiFormatChunker` against the shared chunker contract, for every format and for policies
//! that force splitting.

use nlmx_chunker_structural::{HeuristicTokenCounter, MultiFormatChunker};
use nlmx_domain::{
    document_type::DocumentType,
    ingestion::ChunkPolicy,
    parsed::{
        ChunkContext, ContentBlock, ContentKind, DocumentMetadata, DocumentSection, PageSummary,
        ParsedDocument, RecordField,
    },
    source::SourceLocation,
};
use nlmx_testing::{document_chunker_contract, sample_structured_document};

fn context() -> ChunkContext {
    ChunkContext {
        document_id: 7,
        document_title: Some("Título".into()),
        file_name: Some("arquivo.ext".into()),
        language: Some("pt-BR".into()),
    }
}

fn small_policy() -> ChunkPolicy {
    ChunkPolicy {
        target_tokens: 24,
        max_tokens: 40,
        overlap_tokens: 6,
        min_tokens: 8,
    }
}

fn csv_document(rows: u32) -> ParsedDocument {
    let blocks = (1..=rows)
        .map(|row| {
            let fields = vec![
                RecordField {
                    name: "Nome".into(),
                    value: format!("Pessoa{row}"),
                },
                RecordField {
                    name: "Cidade".into(),
                    value: "Recife".into(),
                },
            ];
            ContentBlock {
                text: format!("Registro {row}:\nNome: Pessoa{row}\nCidade: Recife"),
                kind: ContentKind::Record { fields },
                location: SourceLocation::csv(row, row).unwrap(),
            }
        })
        .collect();
    ParsedDocument::new(
        DocumentType::Csv,
        DocumentMetadata::default(),
        vec![],
        vec![DocumentSection::new(None, 0, vec![], blocks).unwrap()],
        vec![],
    )
    .unwrap()
}

#[test]
fn the_multiformat_chunker_honours_the_contract() {
    let policies = [ChunkPolicy::default(), small_policy()];
    for policy in &policies {
        for kind in DocumentType::ALL {
            let document = if kind == DocumentType::Csv {
                csv_document(60)
            } else {
                sample_structured_document(kind, 40)
            };
            document_chunker_contract(
                &MultiFormatChunker,
                &document,
                &context(),
                policy,
                &HeuristicTokenCounter,
            );
        }
    }
}

#[test]
fn a_pdf_document_without_body_text_has_no_chunks() {
    let empty = ParsedDocument::new(
        DocumentType::Pdf,
        DocumentMetadata::default(),
        vec![PageSummary {
            number: 1,
            width: 612.0,
            height: 792.0,
            char_count: 0,
            has_text: false,
        }],
        vec![],
        vec![],
    )
    .unwrap();
    document_chunker_contract(
        &MultiFormatChunker,
        &empty,
        &context(),
        &ChunkPolicy::default(),
        &HeuristicTokenCounter,
    );
}
