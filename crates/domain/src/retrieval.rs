//! Hybrid retrieval concepts and pure policies: query normalization, the FTS query, and the fusion
//! of semantic and lexical results with normalized, weighted scores.

use std::{collections::HashMap, fmt};

use crate::{ingestion::DocumentId, vectors::ChunkId};

pub type CollectionId = i64;

/// Inclusive page interval; a chunk matches when its pages intersect it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRange {
    pub from: u32,
    pub to: u32,
}

/// Restrictions combined with AND. `None` means "no restriction".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetrievalFilter {
    pub documents: Option<Vec<DocumentId>>,
    pub collections: Option<Vec<CollectionId>>,
    pub pages: Option<PageRange>,
}

impl RetrievalFilter {
    pub fn is_empty(&self) -> bool {
        self.documents.is_none() && self.collections.is_none() && self.pages.is_none()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetrievalOptions {
    pub top_k: usize,
    pub semantic_weight: f32,
    pub lexical_weight: f32,
    pub filter: RetrievalFilter,
}

impl Default for RetrievalOptions {
    fn default() -> Self {
        Self {
            top_k: 8,
            semantic_weight: 0.6,
            lexical_weight: 0.4,
            filter: RetrievalFilter::default(),
        }
    }
}

pub const MAX_TOP_K: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidOptions(pub String);

impl fmt::Display for InvalidOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl RetrievalOptions {
    pub fn validate(&self) -> Result<(), InvalidOptions> {
        if !(1..=MAX_TOP_K).contains(&self.top_k) {
            return Err(InvalidOptions(format!(
                "top_k deve estar entre 1 e {MAX_TOP_K}"
            )));
        }
        let ok = |w: f32| w.is_finite() && w >= 0.0;
        if !ok(self.semantic_weight) || !ok(self.lexical_weight) {
            return Err(InvalidOptions("os pesos devem ser números ≥ 0".into()));
        }
        if self.semantic_weight + self.lexical_weight == 0.0 {
            return Err(InvalidOptions(
                "pelo menos um dos pesos deve ser maior que zero".into(),
            ));
        }
        if let Some(PageRange { from, to }) = self.filter.pages {
            if from == 0 || to < from {
                return Err(InvalidOptions("intervalo de páginas inválido".into()));
            }
        }
        Ok(())
    }

    /// Candidates fetched from each mechanism before fusion.
    pub fn candidates(&self) -> usize {
        (self.top_k * 4).max(20)
    }
}

/// Collapses whitespace, expands ligatures and drops control characters.
pub fn normalize_query(query: &str) -> String {
    let expanded: String = query
        .chars()
        .flat_map(|c| -> Vec<char> {
            match c {
                'ﬁ' => vec!['f', 'i'],
                'ﬂ' => vec!['f', 'l'],
                'ﬀ' => vec!['f', 'f'],
                'ﬃ' => vec!['f', 'f', 'i'],
                'ﬄ' => vec!['f', 'f', 'l'],
                c if c.is_control() => vec![' '],
                c => vec![c],
            }
        })
        .collect();
    expanded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Short, very frequent Portuguese/English words that only add noise to lexical matching.
const STOPWORDS: &[&str] = &[
    "a", "o", "as", "os", "um", "uma", "de", "do", "da", "dos", "das", "em", "no", "na", "nos",
    "nas", "e", "ou", "que", "qual", "quais", "é", "são", "se", "por", "para", "com", "sem", "ao",
    "aos", "à", "às", "como", "quando", "onde", "meu", "minha", "the", "an", "of", "to", "in",
    "on", "and", "or", "is", "are", "what", "which", "how", "for", "with", "do", "does", "can",
    "i",
];

/// Terms for the FTS5 query, already safe: each term is quoted, so operators such as `AND`,
/// `NEAR(`, `*` or `"` in user text are plain words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexicalQuery {
    pub terms: Vec<String>,
}

impl LexicalQuery {
    pub fn from_query(query: &str) -> Self {
        let mut terms: Vec<String> = Vec::new();
        for token in normalize_query(query).split(|c: char| !c.is_alphanumeric()) {
            let token = token.to_lowercase();
            if token.chars().count() < 2 && !token.chars().all(|c| c.is_ascii_digit())
                || token.is_empty()
            {
                continue;
            }
            if STOPWORDS.contains(&token.as_str()) || terms.contains(&token) {
                continue;
            }
            terms.push(token);
        }
        Self { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// `"termo1" OR "termo2"` (any term matches; BM25 ranks documents with more/rarer terms higher).
    pub fn to_fts5(&self) -> String {
        self.terms
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ")
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SemanticCandidate {
    pub chunk_id: ChunkId,
    pub document_id: DocumentId,
    pub similarity: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LexicalCandidate {
    pub chunk_id: ChunkId,
    pub document_id: DocumentId,
    /// SQLite `bm25()`: lower (more negative) is better.
    pub bm25: f64,
}

/// One mechanism's contribution to a result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ComponentScore {
    /// Cosine similarity (semantic) or −bm25 (lexical).
    pub raw: f32,
    /// In [0, 1].
    pub normalized: f32,
    /// 1-based position in that mechanism's list.
    pub rank: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FusedHit {
    pub chunk_id: ChunkId,
    pub document_id: DocumentId,
    pub score: f32,
    pub semantic: Option<ComponentScore>,
    pub lexical: Option<ComponentScore>,
}

/// Union of both lists with normalized scores (semantic: cosine clamped to [0, 1]; lexical:
/// min-max of −bm25 over its candidates, multiplied by the square of the chunk's query-term
/// `coverage` — so a chunk matching one of three terms cannot get a full lexical score), combined as
/// `(ws·s + wl·l) / (ws + wl)`. Chunks missing from `coverage` count as full coverage.
/// Ties: higher semantic score, then lower chunk id. Returns the best `top_k`.
pub fn fuse(
    semantic: &[SemanticCandidate],
    lexical: &[LexicalCandidate],
    coverage: &HashMap<ChunkId, f32>,
    semantic_weight: f32,
    lexical_weight: f32,
    top_k: usize,
) -> Vec<FusedHit> {
    let mut hits: HashMap<ChunkId, FusedHit> = HashMap::new();
    let entry = |hits: &mut HashMap<ChunkId, FusedHit>, chunk_id, document_id| {
        *hits.entry(chunk_id).or_insert(FusedHit {
            chunk_id,
            document_id,
            score: 0.0,
            semantic: None,
            lexical: None,
        })
    };

    for (i, c) in semantic.iter().enumerate() {
        let mut hit = entry(&mut hits, c.chunk_id, c.document_id);
        hit.semantic = Some(ComponentScore {
            raw: c.similarity,
            normalized: c.similarity.clamp(0.0, 1.0),
            rank: i + 1,
        });
        hits.insert(c.chunk_id, hit);
    }

    let raw: Vec<f64> = lexical.iter().map(|c| -c.bm25).collect();
    let (min, max) = raw
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &x| {
            (lo.min(x), hi.max(x))
        });
    for (i, c) in lexical.iter().enumerate() {
        let spread = if max - min > f64::EPSILON {
            (raw[i] - min) / (max - min)
        } else {
            1.0
        };
        // Squared coverage: all query terms keep the score, 2 of 3 terms ⇒ 0.44, 1 of 3 ⇒ 0.11.
        let covered = f64::from(
            coverage
                .get(&c.chunk_id)
                .copied()
                .unwrap_or(1.0)
                .clamp(0.0, 1.0),
        );
        let normalized = spread * covered * covered;
        let mut hit = entry(&mut hits, c.chunk_id, c.document_id);
        hit.lexical = Some(ComponentScore {
            raw: raw[i] as f32,
            normalized: normalized as f32,
            rank: i + 1,
        });
        hits.insert(c.chunk_id, hit);
    }

    let total = semantic_weight + lexical_weight;
    let mut fused: Vec<FusedHit> = hits
        .into_values()
        .map(|mut hit| {
            let s = hit.semantic.map_or(0.0, |c| c.normalized);
            let l = hit.lexical.map_or(0.0, |c| c.normalized);
            hit.score = if total > 0.0 {
                (semantic_weight * s + lexical_weight * l) / total
            } else {
                0.0
            };
            hit
        })
        .collect();
    fused.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| {
                let s = |h: &FusedHit| h.semantic.map_or(-1.0, |c| c.normalized);
                s(b).total_cmp(&s(a))
            })
            .then(a.chunk_id.cmp(&b.chunk_id))
    });
    fused.truncate(top_k);
    fused
}

/// Lowercase without Portuguese/Spanish/French diacritics (for term matching and highlighting).
pub fn fold(word: &str) -> String {
    word.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            c => c,
        })
        .collect()
}

fn folded_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(fold)
        .collect()
}

/// Fraction of query `terms` that appear as words in `text` (accent/case-insensitive).
pub fn coverage(text: &str, terms: &[String]) -> f32 {
    if terms.is_empty() {
        return 1.0;
    }
    let words: std::collections::HashSet<String> = folded_words(text).into_iter().collect();
    let found = terms.iter().filter(|t| words.contains(&fold(t))).count();
    found as f32 / terms.len() as f32
}

/// Jaccard similarity of word trigrams (folded), in [0, 1]. Texts under three words compare whole.
pub fn jaccard_trigrams(a: &str, b: &str) -> f32 {
    let grams = |text: &str| -> std::collections::HashSet<String> {
        let words = folded_words(text);
        if words.len() < 3 {
            return std::iter::once(words.join(" ")).collect();
        }
        words.windows(3).map(|w| w.join(" ")).collect()
    };
    let (ga, gb) = (grams(a), grams(b));
    let union = ga.union(&gb).count();
    if union == 0 {
        1.0
    } else {
        ga.intersection(&gb).count() as f32 / union as f32
    }
}

/// Text equality ignoring case, accents, punctuation and spacing (exact duplicates).
pub fn same_content(a: &str, b: &str) -> bool {
    folded_words(a) == folded_words(b)
}

/// Whether two texts mention the same numbers (in the same order). Near-duplicate detection
/// requires it: templated passages that differ only in values ("seção 58" vs "seção 137",
/// prices, dates) are different content.
pub fn same_numbers(a: &str, b: &str) -> bool {
    let numbers = |t: &str| -> Vec<String> {
        folded_words(t)
            .into_iter()
            .filter(|w| w.chars().any(|c| c.is_ascii_digit()))
            .collect()
    };
    numbers(a) == numbers(b)
}

/// Whether every word of `inner` appears, in order and contiguously, inside `outer` (folded).
pub fn contains_content(outer: &str, inner: &str) -> bool {
    let (outer, inner) = (folded_words(outer), folded_words(inner));
    !inner.is_empty() && outer.windows(inner.len()).any(|w| w == inner.as_slice())
}

/// Joins two consecutive chunks, dropping the overlap the chunker repeated at the start of `next`
/// (the longest word sequence that ends `first` and starts `next`).
pub fn join_adjacent(first: &str, next: &str) -> String {
    let a: Vec<&str> = first.split_whitespace().collect();
    let b: Vec<&str> = next.split_whitespace().collect();
    let max = a.len().min(b.len());
    let overlap = (1..=max)
        .rev()
        .find(|&n| a[a.len() - n..] == b[..n])
        .unwrap_or(0);
    let rest = b[overlap..].join(" ");
    match (first.trim().is_empty(), rest.is_empty()) {
        (_, true) => first.trim().to_string(),
        (true, false) => rest,
        (false, false) => format!("{}\n\n{rest}", first.trim()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sem(chunk: ChunkId, similarity: f32) -> SemanticCandidate {
        SemanticCandidate {
            chunk_id: chunk,
            document_id: 1,
            similarity,
        }
    }

    fn lex(chunk: ChunkId, bm25: f64) -> LexicalCandidate {
        LexicalCandidate {
            chunk_id: chunk,
            document_id: 1,
            bm25,
        }
    }

    fn order(hits: &[FusedHit]) -> Vec<ChunkId> {
        hits.iter().map(|h| h.chunk_id).collect()
    }

    #[test]
    fn templated_texts_with_different_numbers_are_not_the_same() {
        assert!(same_numbers(
            "Prazo de 30 dias, cláusula 4.",
            "prazo de 30 DIAS — cláusula 4"
        ));
        assert!(!same_numbers(
            "Linha 4 da seção 137.",
            "Linha 4 da seção 58."
        ));
        assert!(same_numbers("sem números", "também sem"));
    }

    #[test]
    fn normalizes_queries() {
        assert_eq!(
            normalize_query("  Qual   é a e\u{FB03}cácia\t?\n "),
            "Qual é a efficácia ?"
        );
        assert_eq!(normalize_query("   "), "");
    }

    #[test]
    fn builds_a_safe_fts_query() {
        let q = LexicalQuery::from_query("Qual é o prazo de carência? Prazo!");
        assert_eq!(
            q.terms,
            ["prazo", "carência"],
            "stopwords and duplicates removed"
        );
        assert_eq!(q.to_fts5(), r#""prazo" OR "carência""#);

        let tricky = LexicalQuery::from_query(r#"NEAR(a b) AND "x* OR y" 465"#);
        // AND/OR are stopwords, single letters are dropped, the rest is quoted.
        assert_eq!(
            tricky.to_fts5(),
            r#""near" OR "465""#,
            "operators become plain quoted terms"
        );
        assert_eq!(
            LexicalQuery::from_query(r#"diz "olá""#).to_fts5(),
            r#""diz" OR "olá""#
        );
        assert!(LexicalQuery::from_query("o que é?").is_empty());
    }

    #[test]
    fn validates_options() {
        assert!(RetrievalOptions::default().validate().is_ok());
        let bad = |o: RetrievalOptions| o.validate().is_err();
        assert!(bad(RetrievalOptions {
            top_k: 0,
            ..Default::default()
        }));
        assert!(bad(RetrievalOptions {
            top_k: 101,
            ..Default::default()
        }));
        assert!(bad(RetrievalOptions {
            semantic_weight: 0.0,
            lexical_weight: 0.0,
            ..Default::default()
        }));
        assert!(bad(RetrievalOptions {
            lexical_weight: -1.0,
            ..Default::default()
        }));
        let pages = |from, to| RetrievalOptions {
            filter: RetrievalFilter {
                pages: Some(PageRange { from, to }),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(bad(pages(3, 2)) && bad(pages(0, 1)) && !bad(pages(2, 2)));
    }

    #[test]
    fn fuses_with_normalized_weighted_scores() {
        // Semantic: 1 (0.9), 2 (0.5). Lexical: 3 (best, -bm25 = 6), 2 (2), 4 (worst, 1).
        let semantic = [sem(1, 0.9), sem(2, 0.5)];
        let lexical = [lex(3, -6.0), lex(2, -2.0), lex(4, -1.0)];
        let hits = fuse(&semantic, &lexical, &HashMap::new(), 0.5, 0.5, 10);

        let by_id = |id| *hits.iter().find(|h| h.chunk_id == id).unwrap();
        // Lexical min-max: 6 → 1.0, 2 → 0.2, 1 → 0.0.
        assert!((by_id(3).lexical.unwrap().normalized - 1.0).abs() < 1e-6);
        assert!((by_id(2).lexical.unwrap().normalized - 0.2).abs() < 1e-6);
        assert!((by_id(4).lexical.unwrap().normalized).abs() < 1e-6);
        // Scores: 1 → 0.45, 2 → (0.5 + 0.2)/2 = 0.35, 3 → 0.5, 4 → 0.
        assert!((by_id(1).score - 0.45).abs() < 1e-6);
        assert!((by_id(2).score - 0.35).abs() < 1e-6);
        assert!((by_id(3).score - 0.5).abs() < 1e-6);
        assert_eq!(order(&hits), [3, 1, 2, 4]);
        assert_eq!(by_id(2).semantic.unwrap().rank, 2);
        assert_eq!(by_id(2).lexical.unwrap().rank, 2);
        assert!(by_id(3).semantic.is_none() && by_id(1).lexical.is_none());
    }

    #[test]
    fn weights_select_one_mechanism_or_blend_both() {
        let semantic = [sem(1, 0.9), sem(2, 0.8), sem(3, 0.1)];
        let lexical = [lex(3, -9.0), lex(2, -5.0), lex(1, -1.0)];
        assert_eq!(
            order(&fuse(&semantic, &lexical, &HashMap::new(), 1.0, 0.0, 3)),
            [1, 2, 3],
            "semantic only"
        );
        assert_eq!(
            order(&fuse(&semantic, &lexical, &HashMap::new(), 0.0, 1.0, 3)),
            [3, 2, 1],
            "lexical only"
        );
        assert_eq!(
            order(&fuse(&semantic, &lexical, &HashMap::new(), 0.5, 0.5, 3))[0],
            2,
            "the chunk good in both wins"
        );
        assert_eq!(
            fuse(&semantic, &lexical, &HashMap::new(), 0.5, 0.5, 2).len(),
            2,
            "top_k"
        );
    }

    #[test]
    fn ties_prefer_semantic_then_chunk_id() {
        let hits = fuse(
            &[sem(7, 0.5)],
            &[lex(5, -3.0)],
            &HashMap::new(),
            1.0,
            0.5,
            10,
        );
        // 7: 0.5/1.5·1.0 = 0.333; 5: 0.5·1.0/1.5 = 0.333 → semantic wins the tie.
        assert_eq!(order(&hits), [7, 5]);
        let same = fuse(
            &[sem(9, 0.4), sem(8, 0.4)],
            &[],
            &HashMap::new(),
            1.0,
            0.0,
            10,
        );
        assert_eq!(order(&same), [8, 9]);
        assert_eq!(
            fuse(&[], &[lex(1, -2.0)], &HashMap::new(), 0.5, 0.5, 10)[0]
                .lexical
                .unwrap()
                .normalized,
            1.0,
            "single candidate"
        );
    }

    #[test]
    fn coverage_counts_query_terms_present() {
        let terms = vec![
            "sinais".to_string(),
            "vitais".to_string(),
            "capturados".to_string(),
        ];
        assert!((coverage("Os SINAIS são capturados.", &terms) - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(coverage("nada aqui", &terms), 0.0);
        assert_eq!(
            coverage("Carência", &["carencia".into()]),
            1.0,
            "accent-insensitive"
        );
        assert_eq!(coverage("qualquer", &[]), 1.0);
    }

    #[test]
    fn weak_lexical_matches_no_longer_beat_strong_semantic_ones() {
        // The real case: chunk 1 matched 1 of 3 query terms (lexical best ⇒ 100 % before),
        // chunk 2 is the right passage with a clearly better semantic score and no lexical match.
        let semantic = [sem(1, 0.63), sem(2, 0.78)];
        let lexical = [lex(1, -3.0), lex(3, -1.0)];
        let without = fuse(&semantic, &lexical, &HashMap::new(), 0.6, 0.4, 3);
        assert_eq!(
            without[0].chunk_id, 1,
            "the defect: min-max gave the weak match a full score"
        );
        let coverage = HashMap::from([(1, 1.0 / 3.0), (3, 1.0 / 3.0)]);
        let with = fuse(&semantic, &lexical, &coverage, 0.6, 0.4, 3);
        assert_eq!(with[0].chunk_id, 2);
        assert!(
            (with
                .iter()
                .find(|h| h.chunk_id == 1)
                .unwrap()
                .lexical
                .unwrap()
                .normalized
                - 1.0 / 9.0)
                .abs()
                < 1e-6
        );
        // Full coverage keeps the lexical score untouched.
        let full = fuse(&semantic, &lexical, &HashMap::from([(1, 1.0)]), 0.6, 0.4, 3);
        assert_eq!(full[0].chunk_id, 1);
    }

    #[test]
    fn similarity_helpers() {
        assert!(same_content(
            "A Carência é de 180 dias.",
            "a carencia e de 180 dias"
        ));
        assert!(!same_content(
            "A carência é de 180 dias.",
            "A carência é de 90 dias."
        ));
        assert_eq!(
            jaccard_trigrams("um dois três quatro", "um dois três quatro"),
            1.0
        );
        let near = jaccard_trigrams(
            "o prazo de carência para internação é de cento e oitenta dias",
            "o prazo de carência para internação é de cento e oitenta dias corridos",
        );
        assert!(near > 0.8, "{near}");
        assert!(
            jaccard_trigrams(
                "cobertura de exames laboratoriais",
                "rescisão do contrato com aviso"
            ) < 0.1
        );
    }

    #[test]
    fn joins_adjacent_chunks_without_repeating_the_overlap() {
        assert_eq!(
            join_adjacent(
                "Primeira parte termina assim. Frase de overlap aqui.",
                "Frase de overlap aqui. E continua depois."
            ),
            "Primeira parte termina assim. Frase de overlap aqui.\n\nE continua depois."
        );
        assert_eq!(
            join_adjacent("Sem overlap.", "Outro trecho."),
            "Sem overlap.\n\nOutro trecho."
        );
        assert_eq!(
            join_adjacent("Tudo repetido aqui.", "Tudo repetido aqui."),
            "Tudo repetido aqui."
        );
    }
}
