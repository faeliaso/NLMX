//! Local performance measurements. Private by construction: a measurement holds only ids,
//! counts, durations and fixed labels — never document text, questions, answers, file names
//! or paths — so nothing sensitive can reach logs or diagnostics through it.

use crate::ingestion::DocumentId;

/// Why an operation failed, without its (possibly sensitive) message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Timeout,
    Unavailable,
    Corrupt,
    Refused,
    NotFound,
    Storage,
    Invalid,
    Other,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Unavailable => "unavailable",
            Self::Corrupt => "corrupt",
            Self::Refused => "refused",
            Self::NotFound => "not_found",
            Self::Storage => "storage",
            Self::Invalid => "invalid",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestStage {
    Read,
    Extract,
    Structure,
    Chunk,
    Save,
}

impl IngestStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Extract => "extract",
            Self::Structure => "structure",
            Self::Chunk => "chunk",
            Self::Save => "save",
        }
    }
}

/// The operations whose duration and error rate are tracked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Operation {
    Ingest,
    Embed,
    Retrieve,
    Generate,
}

impl Operation {
    pub const ALL: [Operation; 4] = [Self::Ingest, Self::Embed, Self::Retrieve, Self::Generate];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ingest => "ingest",
            Self::Embed => "embed",
            Self::Retrieve => "retrieve",
            Self::Generate => "generate",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|o| o.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Measurement {
    Ingested {
        document_id: DocumentId,
        bytes: u64,
        pages: u32,
        chunks: u32,
        extract_ms: u64,
        structure_ms: u64,
        chunk_ms: u64,
        save_ms: u64,
        total_ms: u64,
    },
    IngestFailed {
        document_id: DocumentId,
        stage: IngestStage,
        kind: ErrorKind,
        total_ms: u64,
    },
    Embedded {
        document_id: DocumentId,
        chunks: u32,
        dims: u32,
        total_ms: u64,
    },
    EmbedFailed {
        document_id: DocumentId,
        kind: ErrorKind,
        total_ms: u64,
    },
    Retrieved {
        mode: &'static str,
        candidates: u32,
        passages: u32,
        semantic_ms: u64,
        lexical_ms: u64,
        total_ms: u64,
    },
    RetrieveFailed {
        kind: ErrorKind,
        total_ms: u64,
    },
    Generated {
        intent: &'static str,
        status: &'static str,
        prompt_tokens: u32,
        /// Time to the first streamed token.
        first_token_ms: Option<u64>,
        output_chars: u32,
        total_ms: u64,
    },
    GenerationFailed {
        kind: ErrorKind,
        total_ms: u64,
    },
    Resources {
        rss_bytes: u64,
        llama_rss_bytes: Option<u64>,
        fm_rss_bytes: Option<u64>,
    },
    Storage {
        database_bytes: u64,
        library_bytes: u64,
        models_bytes: u64,
        logs_bytes: u64,
    },
}

/// A field value: numbers and fixed labels only.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    U64(u64),
    I64(i64),
    Label(&'static str),
    None,
}

impl Measurement {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Ingested { .. } => "ingested",
            Self::IngestFailed { .. } => "ingest_failed",
            Self::Embedded { .. } => "embedded",
            Self::EmbedFailed { .. } => "embed_failed",
            Self::Retrieved { .. } => "retrieved",
            Self::RetrieveFailed { .. } => "retrieve_failed",
            Self::Generated { .. } => "generated",
            Self::GenerationFailed { .. } => "generation_failed",
            Self::Resources { .. } => "resources",
            Self::Storage { .. } => "storage",
        }
    }

    /// The tracked operation, its duration and whether it failed.
    pub fn outcome(&self) -> Option<(Operation, u64, bool)> {
        match *self {
            Self::Ingested { total_ms, .. } => Some((Operation::Ingest, total_ms, false)),
            Self::IngestFailed { total_ms, .. } => Some((Operation::Ingest, total_ms, true)),
            Self::Embedded { total_ms, .. } => Some((Operation::Embed, total_ms, false)),
            Self::EmbedFailed { total_ms, .. } => Some((Operation::Embed, total_ms, true)),
            Self::Retrieved { total_ms, .. } => Some((Operation::Retrieve, total_ms, false)),
            Self::RetrieveFailed { total_ms, .. } => Some((Operation::Retrieve, total_ms, true)),
            Self::Generated { total_ms, .. } => Some((Operation::Generate, total_ms, false)),
            Self::GenerationFailed { total_ms, .. } => Some((Operation::Generate, total_ms, true)),
            Self::Resources { .. } | Self::Storage { .. } => None,
        }
    }

    pub fn fields(&self) -> Vec<(&'static str, Value)> {
        use Value::*;
        let opt = |v: Option<u64>| v.map_or(None, U64);
        match *self {
            Self::Ingested {
                document_id,
                bytes,
                pages,
                chunks,
                extract_ms,
                structure_ms,
                chunk_ms,
                save_ms,
                total_ms,
            } => vec![
                ("document_id", I64(document_id)),
                ("bytes", U64(bytes)),
                ("pages", U64(pages.into())),
                ("chunks", U64(chunks.into())),
                ("extract_ms", U64(extract_ms)),
                ("structure_ms", U64(structure_ms)),
                ("chunk_ms", U64(chunk_ms)),
                ("save_ms", U64(save_ms)),
                ("total_ms", U64(total_ms)),
            ],
            Self::IngestFailed {
                document_id,
                stage,
                kind,
                total_ms,
            } => vec![
                ("document_id", I64(document_id)),
                ("stage", Label(stage.as_str())),
                ("kind", Label(kind.as_str())),
                ("total_ms", U64(total_ms)),
            ],
            Self::Embedded {
                document_id,
                chunks,
                dims,
                total_ms,
            } => vec![
                ("document_id", I64(document_id)),
                ("chunks", U64(chunks.into())),
                ("dims", U64(dims.into())),
                ("total_ms", U64(total_ms)),
            ],
            Self::EmbedFailed {
                document_id,
                kind,
                total_ms,
            } => vec![
                ("document_id", I64(document_id)),
                ("kind", Label(kind.as_str())),
                ("total_ms", U64(total_ms)),
            ],
            Self::Retrieved {
                mode,
                candidates,
                passages,
                semantic_ms,
                lexical_ms,
                total_ms,
            } => vec![
                ("mode", Label(mode)),
                ("candidates", U64(candidates.into())),
                ("passages", U64(passages.into())),
                ("semantic_ms", U64(semantic_ms)),
                ("lexical_ms", U64(lexical_ms)),
                ("total_ms", U64(total_ms)),
            ],
            Self::RetrieveFailed { kind, total_ms } => {
                vec![("kind", Label(kind.as_str())), ("total_ms", U64(total_ms))]
            }
            Self::Generated {
                intent,
                status,
                prompt_tokens,
                first_token_ms,
                output_chars,
                total_ms,
            } => vec![
                ("intent", Label(intent)),
                ("status", Label(status)),
                ("prompt_tokens", U64(prompt_tokens.into())),
                ("first_token_ms", opt(first_token_ms)),
                ("output_chars", U64(output_chars.into())),
                ("total_ms", U64(total_ms)),
            ],
            Self::GenerationFailed { kind, total_ms } => {
                vec![("kind", Label(kind.as_str())), ("total_ms", U64(total_ms))]
            }
            Self::Resources {
                rss_bytes,
                llama_rss_bytes,
                fm_rss_bytes,
            } => vec![
                ("rss_bytes", U64(rss_bytes)),
                ("llama_rss_bytes", opt(llama_rss_bytes)),
                ("fm_rss_bytes", opt(fm_rss_bytes)),
            ],
            Self::Storage {
                database_bytes,
                library_bytes,
                models_bytes,
                logs_bytes,
            } => vec![
                ("database_bytes", U64(database_bytes)),
                ("library_bytes", U64(library_bytes)),
                ("models_bytes", U64(models_bytes)),
                ("logs_bytes", U64(logs_bytes)),
            ],
        }
    }

    /// Compact JSON object (labels are fixed ASCII identifiers: no escaping needed).
    pub fn to_json(&self) -> String {
        let body: Vec<String> = self
            .fields()
            .into_iter()
            .map(|(k, v)| match v {
                Value::U64(n) => format!("\"{k}\":{n}"),
                Value::I64(n) => format!("\"{k}\":{n}"),
                Value::Label(s) => format!("\"{k}\":\"{s}\""),
                Value::None => format!("\"{k}\":null"),
            })
            .collect();
        format!("{{{}}}", body.join(","))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_numbers_and_labels_only() {
        let m = Measurement::Generated {
            intent: "regular",
            status: "answered",
            prompt_tokens: 812,
            first_token_ms: Some(640),
            output_chars: 233,
            total_ms: 2100,
        };
        assert_eq!(
            m.to_json(),
            r#"{"intent":"regular","status":"answered","prompt_tokens":812,"first_token_ms":640,"output_chars":233,"total_ms":2100}"#
        );
        assert_eq!(m.outcome(), Some((Operation::Generate, 2100, false)));
        let failed = Measurement::IngestFailed {
            document_id: 3,
            stage: IngestStage::Extract,
            kind: ErrorKind::Corrupt,
            total_ms: 5,
        };
        assert_eq!(
            failed.to_json(),
            r#"{"document_id":3,"stage":"extract","kind":"corrupt","total_ms":5}"#
        );
        assert!(failed.outcome().unwrap().2);
        assert_eq!(
            Measurement::Resources {
                rss_bytes: 1,
                llama_rss_bytes: None,
                fm_rss_bytes: Some(2)
            }
            .to_json(),
            r#"{"rss_bytes":1,"llama_rss_bytes":null,"fm_rss_bytes":2}"#
        );
    }

    #[test]
    fn labels_are_plain_identifiers() {
        // Labels are written unescaped into JSON: keep them [a-z_].
        for kind in [ErrorKind::Timeout, ErrorKind::NotFound, ErrorKind::Other] {
            assert!(
                kind.as_str()
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_')
            );
        }
        for op in Operation::ALL {
            assert_eq!(Operation::parse(op.as_str()), Some(op));
        }
    }
}
