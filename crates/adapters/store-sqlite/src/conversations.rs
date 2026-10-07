//! `ConversationRepository`: conversations, messages and the sources of each answer.

use nlmx_application::ports::{BoxFuture, ConversationRepository, FinishedAnswer, StorageError};
use nlmx_domain::{
    chat::{
        AnswerGrounding, Conversation, ConversationId, ConversationScope, ConversationSummary,
        Message, MessageId, MessagePageRef, MessageSource, MessageStatus, Role,
    },
    ingestion::{DocumentId, PageBox, SECTION_SEPARATOR},
    source::{SourceLocation, SourceReference},
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::{Database, lexical::parse_boxes, sections::location_of};

/// The JSON stored in `citations.locator`. A PDF's boxes live in the `bboxes` column.
fn locator_json(location: &SourceLocation) -> Result<String, StorageError> {
    let location = match location {
        SourceLocation::Pdf {
            page_start,
            page_end,
            ..
        } => SourceLocation::Pdf {
            page_start: *page_start,
            page_end: *page_end,
            boxes: Vec::new(),
        },
        other => other.clone(),
    };
    serde_json::to_string(&location)
        .map_err(|e| StorageError::new(format!("Falha ao gravar as fontes: {e}")))
}

fn err(action: &str) -> impl Fn(rusqlite::Error) -> StorageError + '_ {
    move |e| StorageError::new(format!("Falha ao {action}: {e}"))
}

/// (`status`, `outcome`) columns of a message.
fn status_columns(status: MessageStatus) -> (&'static str, Option<&'static str>) {
    match status {
        MessageStatus::Streaming => ("streaming", None),
        MessageStatus::Answered => ("complete", Some("answered")),
        MessageStatus::NotFound => ("no_answer", Some("not_found")),
        MessageStatus::Refused => ("no_answer", Some("refused")),
        MessageStatus::Cancelled => ("cancelled", Some("cancelled")),
        MessageStatus::Failed => ("failed", Some("error")),
    }
}

fn parse_status(status: &str, outcome: Option<&str>) -> MessageStatus {
    match (status, outcome) {
        ("streaming", _) => MessageStatus::Streaming,
        (_, Some("refused")) => MessageStatus::Refused,
        ("no_answer", _) => MessageStatus::NotFound,
        ("cancelled", _) => MessageStatus::Cancelled,
        ("failed", _) => MessageStatus::Failed,
        _ => MessageStatus::Answered,
    }
}

fn boxes_json(boxes: &[PageBox]) -> String {
    let items: Vec<serde_json::Value> = boxes
        .iter()
        .map(|b| {
            serde_json::json!({
                "page": b.page,
                "left": b.bbox.left,
                "top": b.bbox.top,
                "right": b.bbox.right,
                "bottom": b.bbox.bottom,
            })
        })
        .collect();
    serde_json::Value::Array(items).to_string()
}

fn grounding_column(grounding: AnswerGrounding) -> &'static str {
    match grounding {
        AnswerGrounding::Documents => "documents",
        AnswerGrounding::Free => "free",
    }
}

fn parse_grounding(value: Option<&str>) -> Option<AnswerGrounding> {
    match value {
        Some("free") => Some(AnswerGrounding::Free),
        Some(_) => Some(AnswerGrounding::Documents),
        None => None,
    }
}

const CONVERSATION: &str = "SELECT c.id, c.title, c.mode, s.document_id, c.updated_at
     FROM conversations c LEFT JOIN conversation_scopes s ON s.conversation_id = c.id";

fn conversation(r: &rusqlite::Row<'_>) -> rusqlite::Result<Conversation> {
    let mode: String = r.get(2)?;
    let document: Option<DocumentId> = r.get(3)?;
    Ok(Conversation {
        id: r.get(0)?,
        title: r.get(1)?,
        scope: match (mode.as_str(), document) {
            ("free", _) => ConversationScope::Free,
            (_, Some(document)) => ConversationScope::Document(document),
            (_, None) => ConversationScope::Library,
        },
        updated_at: r.get(4)?,
    })
}

fn get(conn: &Connection, id: ConversationId) -> Result<Option<Conversation>, StorageError> {
    conn.query_row(
        &format!("{CONVERSATION} WHERE c.id = ?1"),
        [id],
        conversation,
    )
    .optional()
    .map_err(err("ler a conversa"))
}

/// Sets the mode and the document row; the caller touches the conversation.
fn set_scope(
    conn: &Connection,
    id: ConversationId,
    scope: ConversationScope,
) -> Result<(), StorageError> {
    let mode = match scope {
        ConversationScope::Free => "free",
        ConversationScope::Library | ConversationScope::Document(_) => "documents",
    };
    conn.execute(
        "UPDATE conversations SET mode = ?2 WHERE id = ?1",
        params![id, mode],
    )
    .map_err(err("mudar o escopo"))?;
    conn.execute(
        "DELETE FROM conversation_scopes WHERE conversation_id = ?1",
        [id],
    )
    .map_err(err("mudar o escopo"))?;
    if let Some(document) = scope.document() {
        conn.execute(
            "INSERT INTO conversation_scopes (conversation_id, document_id) VALUES (?1, ?2)",
            params![id, document],
        )
        .map_err(err("mudar o escopo"))?;
    }
    Ok(())
}

fn sources(conn: &Connection, message: MessageId) -> Result<Vec<MessageSource>, StorageError> {
    let mut stmt = conn
        .prepare(
            "SELECT ordinal, cited, document_id, chunk_id, document_title, page_number,
                    coalesce(page_end, page_number), section, label, quote, bboxes,
                    document_name, locator
             FROM citations WHERE message_id = ?1 ORDER BY ordinal",
        )
        .map_err(err("ler fontes"))?;
    stmt.query_map([message], |r| {
        let (document_id, chunk_id, title): (i64, Option<i64>, String) =
            (r.get(2)?, r.get(3)?, r.get(4)?);
        let (page_start, page_end): (u32, u32) = (r.get(5)?, r.get(6)?);
        let section: Option<String> = r.get(7)?;
        let bboxes = parse_boxes(&r.get::<_, String>(10)?);
        // A citation without a locator is a PDF one (from before provenance): its pages.
        let location = location_of(
            r.get::<_, Option<String>>(12)?.as_deref(),
            page_start,
            page_end,
            bboxes.clone(),
        )?;
        let reference = SourceReference {
            document_id,
            document_title: title.clone(),
            chunk_id,
            location,
            section_path: section
                .as_deref()
                .map(|s| s.split(SECTION_SEPARATOR).map(str::to_string).collect())
                .unwrap_or_default(),
        };
        Ok(MessageSource {
            n: r.get(0)?,
            cited: r.get::<_, i64>(1)? == 1,
            document_id,
            chunk_id,
            document_title: title,
            page_start,
            page_end,
            section,
            label: r.get(8)?,
            quote: r.get(9)?,
            bboxes,
            reference,
            document_name: r.get(11)?,
        })
    })
    .and_then(|rows| rows.collect())
    .map_err(err("ler fontes"))
}

const MESSAGE: &str =
    "SELECT id, conversation_id, role, content, status, outcome, error, created_at, grounding
     FROM messages";

fn message_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let status: String = r.get(4)?;
    let outcome: Option<String> = r.get(5)?;
    Ok(Message {
        id: r.get(0)?,
        conversation_id: r.get(1)?,
        role: if r.get::<_, String>(2)? == "user" {
            Role::User
        } else {
            Role::Assistant
        },
        content: r.get(3)?,
        status: parse_status(&status, outcome.as_deref()),
        grounding: parse_grounding(r.get::<_, Option<String>>(8)?.as_deref()),
        error: r.get(6)?,
        sources: Vec::new(),
        page_refs: Vec::new(),
        created_at: r.get(7)?,
    })
}

fn page_refs(conn: &Connection, message: MessageId) -> Result<Vec<MessagePageRef>, StorageError> {
    let mut stmt = conn
        .prepare(
            "SELECT page_number, document_id, source FROM message_page_refs
             WHERE message_id = ?1 ORDER BY ordinal",
        )
        .map_err(err("ler referências de página"))?;
    stmt.query_map([message], |r| {
        Ok(MessagePageRef {
            page: r.get(0)?,
            document_id: r.get(1)?,
            source: r.get(2)?,
        })
    })
    .and_then(|rows| rows.collect())
    .map_err(err("ler referências de página"))
}

fn with_sources(conn: &Connection, mut message: Message) -> Result<Message, StorageError> {
    if message.role == Role::Assistant {
        message.sources = sources(conn, message.id)?;
        message.page_refs = page_refs(conn, message.id)?;
    }
    Ok(message)
}

fn touch(conn: &Connection, id: ConversationId) -> Result<(), StorageError> {
    // The touch trigger bumps `version` and `updated_at`.
    conn.execute("UPDATE conversations SET title = title WHERE id = ?1", [id])
        .map(|_| ())
        .map_err(err("atualizar a conversa"))
}

impl ConversationRepository for Database {
    fn create(
        &self,
        scope: ConversationScope,
    ) -> BoxFuture<'_, Result<Conversation, StorageError>> {
        Box::pin(self.run(move |conn| {
            let tx = conn.transaction().map_err(err("criar a conversa"))?;
            tx.execute("INSERT INTO conversations DEFAULT VALUES", [])
                .map_err(err("criar a conversa"))?;
            let id = tx.last_insert_rowid();
            set_scope(&tx, id, scope)?;
            let created = get(&tx, id)?.expect("just inserted");
            tx.commit().map_err(err("criar a conversa"))?;
            Ok(created)
        }))
    }

    fn get(&self, id: ConversationId) -> BoxFuture<'_, Result<Option<Conversation>, StorageError>> {
        Box::pin(self.run(move |conn| get(conn, id)))
    }

    fn recent(&self, limit: u32) -> BoxFuture<'_, Result<Vec<ConversationSummary>, StorageError>> {
        Box::pin(self.run(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT c.id, c.title, c.updated_at,
                            (SELECT count(*) FROM messages m WHERE m.conversation_id = c.id)
                     FROM conversations c WHERE c.archived_at IS NULL
                     ORDER BY c.updated_at DESC, c.id DESC LIMIT ?1",
                )
                .map_err(err("listar conversas"))?;
            stmt.query_map([limit], |r| {
                Ok(ConversationSummary {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    updated_at: r.get(2)?,
                    messages: r.get(3)?,
                })
            })
            .and_then(|rows| rows.collect())
            .map_err(err("listar conversas"))
        }))
    }

    fn set_scope(
        &self,
        id: ConversationId,
        scope: ConversationScope,
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        Box::pin(self.run(move |conn| {
            let tx = conn.transaction().map_err(err("mudar o escopo"))?;
            set_scope(&tx, id, scope)?;
            touch(&tx, id)?;
            tx.commit().map_err(err("mudar o escopo"))
        }))
    }

    fn set_title<'a>(
        &'a self,
        id: ConversationId,
        title: &'a str,
    ) -> BoxFuture<'a, Result<(), StorageError>> {
        let title = title.to_string();
        Box::pin(self.run(move |conn| {
            conn.execute(
                "UPDATE conversations SET title = ?2 WHERE id = ?1",
                params![id, title],
            )
            .map(|_| ())
            .map_err(err("renomear a conversa"))
        }))
    }

    fn delete(&self, id: ConversationId) -> BoxFuture<'_, Result<(), StorageError>> {
        Box::pin(self.run(move |conn| {
            conn.execute("DELETE FROM conversations WHERE id = ?1", [id])
                .map(|_| ())
                .map_err(err("excluir a conversa"))
        }))
    }

    fn messages(&self, id: ConversationId) -> BoxFuture<'_, Result<Vec<Message>, StorageError>> {
        Box::pin(self.run(move |conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "{MESSAGE} WHERE conversation_id = ?1 ORDER BY created_at, id"
                ))
                .map_err(err("ler mensagens"))?;
            let messages: Vec<Message> = stmt
                .query_map([id], message_row)
                .and_then(|rows| rows.collect())
                .map_err(err("ler mensagens"))?;
            messages
                .into_iter()
                .map(|m| with_sources(conn, m))
                .collect()
        }))
    }

    fn message(&self, id: MessageId) -> BoxFuture<'_, Result<Option<Message>, StorageError>> {
        Box::pin(self.run(move |conn| {
            let message = conn
                .query_row(&format!("{MESSAGE} WHERE id = ?1"), [id], message_row)
                .optional()
                .map_err(err("ler a mensagem"))?;
            message.map(|m| with_sources(conn, m)).transpose()
        }))
    }

    fn add_message<'a>(
        &'a self,
        conversation: ConversationId,
        role: Role,
        content: &'a str,
        status: MessageStatus,
        grounding: Option<AnswerGrounding>,
    ) -> BoxFuture<'a, Result<MessageId, StorageError>> {
        let content = content.to_string();
        Box::pin(self.run(move |conn| {
            let tx = conn.transaction().map_err(err("gravar a mensagem"))?;
            let (status, outcome) = status_columns(status);
            tx.execute(
                "INSERT INTO messages (conversation_id, role, content, status, outcome, grounding)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    conversation,
                    match role {
                        Role::User => "user",
                        Role::Assistant => "assistant",
                    },
                    content,
                    status,
                    outcome,
                    grounding.map(grounding_column)
                ],
            )
            .map_err(err("gravar a mensagem"))?;
            let id = tx.last_insert_rowid();
            touch(&tx, conversation)?;
            tx.commit().map_err(err("gravar a mensagem"))?;
            Ok(id)
        }))
    }

    fn finish_message<'a>(
        &'a self,
        id: MessageId,
        answer: FinishedAnswer<'a>,
    ) -> BoxFuture<'a, Result<(), StorageError>> {
        let content = answer.content.to_string();
        let status = answer.status;
        let error = answer.error.map(String::from);
        let sources = answer.sources.to_vec();
        let refs = answer.page_refs.to_vec();
        Box::pin(self.run(move |conn| {
            let tx = conn.transaction().map_err(err("gravar a resposta"))?;
            let (status, outcome) = status_columns(status);
            tx.execute(
                "UPDATE messages SET content = ?2, status = ?3, outcome = ?4, error = ?5 WHERE id = ?1",
                params![id, content, status, outcome, error],
            )
            .map_err(err("gravar a resposta"))?;
            tx.execute("DELETE FROM citations WHERE message_id = ?1", [id])
                .map_err(err("gravar as fontes"))?;
            {
                let mut insert = tx
                    .prepare(
                        "INSERT INTO citations (message_id, ordinal, cited, chunk_id, document_id,
                             document_title, page_number, page_end, section, label, quote, bboxes,
                             document_type, document_name, locator)
                         VALUES (?1, ?2, ?3,
                             (SELECT id FROM document_chunks WHERE id = ?4),
                             ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                    )
                    .map_err(err("gravar as fontes"))?;
                for s in &sources {
                    insert
                        .execute(params![
                            id,
                            s.n,
                            s.cited as i64,
                            s.chunk_id,
                            s.document_id,
                            s.document_title,
                            s.page_start,
                            s.page_end,
                            s.section,
                            s.label,
                            s.quote,
                            boxes_json(&s.bboxes),
                            s.document_type().as_str(),
                            s.document_name,
                            locator_json(&s.reference.location)?
                        ])
                        .map_err(err("gravar as fontes"))?;
                }
            }
            tx.execute("DELETE FROM message_page_refs WHERE message_id = ?1", [id])
                .map_err(err("gravar as referências de página"))?;
            for (i, r) in refs.iter().enumerate() {
                tx.execute(
                    "INSERT INTO message_page_refs (message_id, ordinal, document_id, page_number, source)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![id, i as i64 + 1, r.document_id, r.page, r.source],
                )
                .map_err(err("gravar as referências de página"))?;
            }
            let conversation: ConversationId = tx
                .query_row(
                    "SELECT conversation_id FROM messages WHERE id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .map_err(err("gravar a resposta"))?;
            touch(&tx, conversation)?;
            tx.commit().map_err(err("gravar a resposta"))
        }))
    }

    fn reset_message(
        &self,
        id: MessageId,
        grounding: AnswerGrounding,
    ) -> BoxFuture<'_, Result<(), StorageError>> {
        Box::pin(self.run(move |conn| {
            let tx = conn.transaction().map_err(err("reiniciar a resposta"))?;
            tx.execute("DELETE FROM citations WHERE message_id = ?1", [id])
                .map_err(err("reiniciar a resposta"))?;
            tx.execute("DELETE FROM message_page_refs WHERE message_id = ?1", [id])
                .map_err(err("reiniciar a resposta"))?;
            tx.execute(
                "UPDATE messages SET content = '', status = 'streaming', outcome = NULL, error = NULL,
                     grounding = ?2
                 WHERE id = ?1",
                params![id, grounding_column(grounding)],
            )
            .map_err(err("reiniciar a resposta"))?;
            tx.commit().map_err(err("reiniciar a resposta"))
        }))
    }
}
