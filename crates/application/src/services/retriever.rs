//! The Retriever: turns hybrid search hits into context for the RAG — duplicates removed,
//! neighbouring chunks joined, limits and Top-K applied, grouped by document. No text generation.

use std::sync::Arc;

use nlmx_domain::{
    ingestion::{DocumentId, PageBox},
    retrieval::{
        ComponentScore, MAX_TOP_K, RetrievalFilter, RetrievalOptions, jaccard_trigrams,
        join_adjacent, normalize_query, same_content, same_numbers,
    },
    source::{RetrievedSource, SourceLocation, SourceReference},
    vectors::ChunkId,
};

use super::retrieval::{HybridRetriever, RetrievalError, RetrievalMode, RetrievedChunk};

#[derive(Debug, Clone, PartialEq)]
pub struct RetrieverOptions {
    /// Passages returned.
    pub top_k: usize,
    /// At most this many passages from one document (keeps the context diverse).
    pub max_per_document: usize,
    /// Passages scoring below this (0–1) are not relevant enough for the context.
    pub min_score: f32,
    pub semantic_weight: f32,
    pub lexical_weight: f32,
    pub filter: RetrievalFilter,
    /// Join hits that are consecutive chunks of the same document into one passage.
    pub merge_adjacent: bool,
    /// Trigram Jaccard similarity from which two passages count as near-duplicates.
    pub near_duplicate_threshold: f32,
}

impl Default for RetrieverOptions {
    fn default() -> Self {
        Self {
            top_k: 6,
            max_per_document: 4,
            min_score: 0.2,
            semantic_weight: 0.6,
            lexical_weight: 0.4,
            filter: RetrievalFilter::default(),
            merge_adjacent: true,
            near_duplicate_threshold: 0.8,
        }
    }
}

impl RetrieverOptions {
    fn search_options(&self) -> RetrievalOptions {
        RetrievalOptions {
            // Over-fetch: duplicates, merges and limits remove candidates.
            top_k: (self.top_k * 3).clamp(20, MAX_TOP_K),
            semantic_weight: self.semantic_weight,
            lexical_weight: self.lexical_weight,
            filter: self.filter.clone(),
        }
    }

    fn validate(&self) -> Result<(), RetrievalError> {
        let invalid = |m: &str| Err(RetrievalError::InvalidOptions(m.into()));
        if !(1..=MAX_TOP_K).contains(&self.top_k) {
            return invalid("top_k deve estar entre 1 e 100");
        }
        if self.max_per_document == 0 {
            return invalid("max_per_document deve ser maior que zero");
        }
        if !(0.0..=1.0).contains(&self.min_score)
            || !(0.0..=1.0).contains(&self.near_duplicate_threshold)
        {
            return invalid("min_score e near_duplicate_threshold devem estar entre 0 e 1");
        }
        self.search_options()
            .validate()
            .map_err(|e| RetrievalError::InvalidOptions(e.0))
    }
}

/// Where a passage comes from, for citations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassageSource {
    pub document_title: String,
    pub page_start: u32,
    pub page_end: u32,
    pub section: Option<String>,
    /// e.g. "relatorio.pdf · pp. 2–3", "guia.md · Instalação › Requisitos" (see
    /// [`RetrievedSource::label`]).
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MatchedBy {
    pub semantic: bool,
    pub lexical: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PassageMetadata {
    /// Every chunk joined into this passage, in reading order.
    pub chunk_ids: Vec<ChunkId>,
    pub section_path: Vec<String>,
    /// For highlighting in the PDF viewer.
    pub bboxes: Vec<PageBox>,
    /// Scores of the main chunk.
    pub semantic: Option<ComponentScore>,
    pub lexical: Option<ComponentScore>,
    pub matched_by: MatchedBy,
    /// The same (or nearly the same) content found elsewhere, left out of the context.
    pub duplicates: Vec<(DocumentId, ChunkId)>,
}

/// One unit of context for the RAG.
#[derive(Debug, Clone, PartialEq)]
pub struct Passage {
    pub document_id: DocumentId,
    /// The main (best-scoring) chunk of the passage.
    pub chunk_id: ChunkId,
    /// First page (the citation page).
    pub page: u32,
    pub content: String,
    pub score: f32,
    pub source: PassageSource,
    /// Where the passage comes from, whatever the format: the same for every consumer
    /// (prompt, citations, interface). The interface decides how to present it.
    pub provenance: RetrievedSource,
    pub metadata: PassageMetadata,
}

impl Passage {
    pub fn document_type(&self) -> nlmx_domain::document_type::DocumentType {
        self.provenance.document_type()
    }

    pub fn location(&self) -> &SourceLocation {
        self.provenance.location()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DocumentGroup {
    pub document_id: DocumentId,
    pub title: String,
    pub best_score: f32,
    /// Indexes into `RetrievalContext::passages`, in page order.
    pub passages: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalContext {
    pub question: String,
    /// Best first.
    pub passages: Vec<Passage>,
    /// Best document first.
    pub documents: Vec<DocumentGroup>,
    pub mode: RetrievalMode,
    pub warnings: Vec<String>,
}

pub struct Retriever {
    hybrid: Arc<HybridRetriever>,
}

/// Maximum chunks joined into one passage.
const MAX_JOINED: usize = 3;

impl Retriever {
    pub fn new(hybrid: Arc<HybridRetriever>) -> Self {
        Self { hybrid }
    }

    pub async fn retrieve(
        &self,
        question: &str,
        options: &RetrieverOptions,
    ) -> Result<RetrievalContext, RetrievalError> {
        options.validate()?;
        let question = normalize_query(question);
        let result = self
            .hybrid
            .retrieve(&question, &options.search_options())
            .await?;
        let hits = drop_exact_duplicates(result.chunks);
        let joined = if options.merge_adjacent {
            join_neighbours(hits)
        } else {
            hits.into_iter().map(single).collect()
        };
        let passages = select(
            drop_near_duplicates(joined, options.near_duplicate_threshold),
            options,
        );
        let documents = group(&passages);
        Ok(RetrievalContext {
            question,
            passages,
            documents,
            mode: result.mode,
            warnings: result.warnings,
        })
    }
}

/// A hit plus the hits it absorbed.
struct Draft {
    members: Vec<RetrievedChunk>,
    duplicates: Vec<(DocumentId, ChunkId)>,
}

impl Draft {
    fn main(&self) -> &RetrievedChunk {
        self.members
            .iter()
            .max_by(|a, b| {
                a.score
                    .total_cmp(&b.score)
                    .then(b.chunk.chunk_id.cmp(&a.chunk.chunk_id))
            })
            .expect("a draft has members")
    }

    fn score(&self) -> f32 {
        self.main().score
    }

    fn content(&self) -> String {
        self.members
            .iter()
            .fold(String::new(), |acc, m| join_adjacent(&acc, &m.chunk.text))
    }
}

/// A hit with the duplicates (other documents, same content) it stands for.
type Hit = (RetrievedChunk, Vec<(DocumentId, ChunkId)>);

fn single((hit, duplicates): Hit) -> Draft {
    Draft {
        members: vec![hit],
        duplicates,
    }
}

/// The same text in several documents (e.g. the same PDF saved twice): keep the best hit and
/// remember where else it appears.
fn drop_exact_duplicates(hits: Vec<RetrievedChunk>) -> Vec<Hit> {
    let mut kept: Vec<Hit> = Vec::new();
    for hit in hits {
        let same = kept.iter_mut().find(|(k, _)| {
            k.chunk.content_hash == hit.chunk.content_hash
                || same_content(&k.chunk.text, &hit.chunk.text)
        });
        match same {
            // Hits arrive best first, so the kept one scores at least as high.
            Some((_, duplicates)) => duplicates.push((hit.chunk.document_id, hit.chunk.chunk_id)),
            None => kept.push((hit, Vec::new())),
        }
    }
    kept
}

/// Consecutive chunks (by ordinal) of the same document and section become one passage, in reading order.
fn join_neighbours(hits: Vec<Hit>) -> Vec<Draft> {
    let mut drafts: Vec<Draft> = Vec::new();
    // Best hits first: each joins an existing passage of its document when it touches one.
    for (hit, duplicates) in hits {
        let (doc, ordinal) = (hit.chunk.document_id, hit.chunk.ordinal);
        let target = drafts.iter_mut().find(|d| {
            let first = &d.members[0].chunk;
            let last = &d.members[d.members.len() - 1].chunk;
            // Only within one section: a neighbour from another section is different content.
            first.document_id == doc
                && first.section == hit.chunk.section
                && d.members.len() < MAX_JOINED
                && (last.ordinal + 1 == ordinal || ordinal + 1 == first.ordinal)
        });
        match target {
            Some(draft) => {
                if ordinal < draft.members[0].chunk.ordinal {
                    draft.members.insert(0, hit);
                } else {
                    draft.members.push(hit);
                }
                draft.duplicates.extend(duplicates);
            }
            None => drafts.push(single((hit, duplicates))),
        }
    }
    drafts
}

/// Passages that say nearly the same thing: keep the better one.
fn drop_near_duplicates(drafts: Vec<Draft>, threshold: f32) -> Vec<Draft> {
    let mut ordered = drafts;
    ordered.sort_by(|a, b| b.score().total_cmp(&a.score()));
    let mut kept: Vec<(Draft, String)> = Vec::new();
    for draft in ordered {
        let content = draft.content();
        match kept.iter_mut().find(|(_, text)| {
            jaccard_trigrams(text, &content) >= threshold && same_numbers(text, &content)
        }) {
            Some((better, _)) => {
                let main = draft.main().chunk.clone();
                better.duplicates.push((main.document_id, main.chunk_id));
                better.duplicates.extend(draft.duplicates);
            }
            None => kept.push((draft, content)),
        }
    }
    kept.into_iter().map(|(d, _)| d).collect()
}

/// The location of a passage made of consecutive chunks: the union of theirs (the first one's
/// when they cannot be merged).
fn merged_location(members: &[RetrievedChunk]) -> SourceLocation {
    let mut location = members[0].chunk.location.clone();
    for member in &members[1..] {
        if let Some(merged) = location.merge(&member.chunk.location) {
            location = merged;
        }
    }
    location
}

fn passage(draft: Draft) -> Passage {
    let main = draft.main().clone();
    let first = &draft.members[0].chunk;
    let page_start = draft
        .members
        .iter()
        .map(|m| m.chunk.page_start)
        .min()
        .unwrap_or(first.page_start);
    let page_end = draft
        .members
        .iter()
        .map(|m| m.chunk.page_end)
        .max()
        .unwrap_or(first.page_end);
    let section = first.section.clone();
    let mut bboxes: Vec<PageBox> = Vec::new();
    for b in draft.members.iter().flat_map(|m| m.chunk.bboxes.iter()) {
        if !bboxes.contains(b) {
            bboxes.push(*b);
        }
    }
    let matched_by = MatchedBy {
        semantic: draft.members.iter().any(|m| m.semantic.is_some()),
        lexical: draft.members.iter().any(|m| m.lexical.is_some()),
    };
    let mut duplicates = draft.duplicates.clone();
    duplicates.sort();
    duplicates.dedup();
    let section_path: Vec<String> = section
        .as_deref()
        .map(|s| s.split(" > ").map(String::from).collect())
        .unwrap_or_default();
    let provenance = RetrievedSource {
        reference: SourceReference {
            document_id: main.chunk.document_id,
            document_title: first.document_title.clone(),
            chunk_id: Some(main.chunk.chunk_id),
            location: merged_location(&draft.members),
            section_path: section_path.clone(),
        },
        document_name: first.document_name.clone(),
        relevance_score: main.score,
        metadata: main.chunk.metadata.clone(),
    };
    Passage {
        document_id: main.chunk.document_id,
        chunk_id: main.chunk.chunk_id,
        page: page_start,
        content: draft.content(),
        score: main.score,
        source: PassageSource {
            label: provenance.label(),
            document_title: first.document_title.clone(),
            page_start,
            page_end,
            section: section.clone(),
        },
        metadata: PassageMetadata {
            chunk_ids: draft.members.iter().map(|m| m.chunk.chunk_id).collect(),
            section_path,
            bboxes,
            semantic: main.semantic,
            lexical: main.lexical,
            matched_by,
            duplicates,
        },
        provenance,
    }
}

/// A passage made of consecutive chunks chosen by document structure rather than by search
/// (document overviews, a numbered section), with score 1.
pub fn structural_passage(chunks: Vec<crate::ports::ChunkView>) -> Passage {
    let members = chunks
        .into_iter()
        .map(|chunk| RetrievedChunk {
            chunk,
            score: 1.0,
            semantic: None,
            lexical: None,
        })
        .collect();
    passage(Draft {
        members,
        duplicates: Vec::new(),
    })
}

/// `min_score`, `max_per_document` and Top-K, best first (ties: semantic score, then chunk id).
fn select(drafts: Vec<Draft>, options: &RetrieverOptions) -> Vec<Passage> {
    let mut passages: Vec<Passage> = drafts
        .into_iter()
        .map(passage)
        .filter(|p| p.score >= options.min_score)
        .collect();
    passages.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| {
                let s = |p: &Passage| p.metadata.semantic.map_or(-1.0, |c| c.normalized);
                s(b).total_cmp(&s(a))
            })
            .then(a.chunk_id.cmp(&b.chunk_id))
    });
    let mut per_document: std::collections::HashMap<DocumentId, usize> =
        std::collections::HashMap::new();
    passages
        .into_iter()
        .filter(|p| {
            let n = per_document.entry(p.document_id).or_default();
            *n += 1;
            *n <= options.max_per_document
        })
        .take(options.top_k)
        .collect()
}

fn group(passages: &[Passage]) -> Vec<DocumentGroup> {
    let mut groups: Vec<DocumentGroup> = Vec::new();
    for (i, p) in passages.iter().enumerate() {
        match groups.iter_mut().find(|g| g.document_id == p.document_id) {
            Some(g) => g.passages.push(i),
            // Passages are best first, so the first one sets the group's best score.
            None => groups.push(DocumentGroup {
                document_id: p.document_id,
                title: p.source.document_title.clone(),
                best_score: p.score,
                passages: vec![i],
            }),
        }
    }
    for g in &mut groups {
        g.passages
            .sort_by_key(|&i| (passages[i].page, passages[i].chunk_id));
    }
    groups
}
