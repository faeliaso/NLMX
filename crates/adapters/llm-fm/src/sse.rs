//! Incremental parser for the `fm serve` chat-completions SSE stream.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseEvent {
    Delta(String),
    /// `finish_reason` of the last chunk ("stop", "length", ...).
    Finish(String),
    /// Final `usage` chunk (requested with `stream_options.include_usage`).
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
    },
    /// `event: error` — e.g. the safety guardrails were triggered.
    Error(String),
    Done,
}

#[derive(Default)]
pub struct SseParser {
    buffer: Vec<u8>,
}

#[derive(Deserialize)]
struct Chunk {
    #[serde(default)]
    choices: Vec<Choice>,
    error: Option<ErrorBody>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    delta: Delta,
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct Delta {
    content: Option<String>,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: String,
}

impl SseParser {
    /// Feeds bytes; returns the events completed by them.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, String> {
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(end) = find_event_end(&self.buffer) {
            let raw: Vec<u8> = self.buffer.drain(..end.0 + end.1).collect();
            let text = String::from_utf8_lossy(&raw[..end.0]).to_string();
            events.extend(parse_event(&text)?);
        }
        Ok(events)
    }
}

/// Position and length of the first blank-line separator.
fn find_event_end(buf: &[u8]) -> Option<(usize, usize)> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2));
    let crlf = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

fn parse_event(text: &str) -> Result<Vec<SseEvent>, String> {
    let mut name = "message";
    let mut data = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("event:") {
            name = v.trim();
        } else if let Some(v) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(v.strip_prefix(' ').unwrap_or(v));
        }
    }
    if data.is_empty() {
        return Ok(Vec::new());
    }
    if data.trim() == "[DONE]" {
        return Ok(vec![SseEvent::Done]);
    }
    let chunk: Chunk =
        serde_json::from_str(&data).map_err(|e| format!("evento SSE inválido: {e}"))?;
    if name == "error" || chunk.error.is_some() {
        let message = chunk
            .error
            .map(|e| e.message)
            .unwrap_or_else(|| "erro desconhecido".into());
        return Ok(vec![SseEvent::Error(message)]);
    }
    let mut events = Vec::new();
    for choice in chunk.choices {
        if let Some(content) = choice.delta.content.filter(|c| !c.is_empty()) {
            events.push(SseEvent::Delta(content));
        }
        if let Some(reason) = choice.finish_reason {
            events.push(SseEvent::Finish(reason));
        }
    }
    if let Some(Usage {
        prompt_tokens: Some(prompt_tokens),
        completion_tokens: Some(completion_tokens),
    }) = chunk.usage
    {
        events.push(SseEvent::Usage {
            prompt_tokens,
            completion_tokens,
        });
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_deltas_finish_and_done_across_split_chunks() {
        let stream = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"A capital\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\" é Brasília.\"}}]}\n\n",
            "data: {\"choices\":[{\"finish_reason\":\"stop\",\"index\":0,\"delta\":{}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let mut parser = SseParser::default();
        let mut events = Vec::new();
        for piece in stream.as_bytes().chunks(7) {
            events.extend(parser.push(piece).unwrap());
        }
        assert_eq!(
            events,
            [
                SseEvent::Delta("A capital".into()),
                SseEvent::Delta(" é Brasília.".into()),
                SseEvent::Finish("stop".into()),
                SseEvent::Done
            ]
        );
    }

    #[test]
    fn usage_chunk_reports_prompt_and_completion_tokens() {
        let mut parser = SseParser::default();
        let events = parser
            .push(b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":64,\"completion_tokens\":121,\"total_tokens\":185}}\n\n")
            .unwrap();
        assert_eq!(
            events,
            [SseEvent::Usage {
                prompt_tokens: 64,
                completion_tokens: 121
            }]
        );
        // A usage object missing a count is ignored rather than completed with a guess.
        let events = parser
            .push(b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":64}}\n\n")
            .unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn guardrail_error_event() {
        let mut parser = SseParser::default();
        let events = parser
            .push(b"event: error\ndata: {\"error\":{\"code\":\"500\",\"message\":\"The model's safety guardrails were triggered.\",\"type\":\"server_error\"}}\n\n")
            .unwrap();
        assert_eq!(
            events,
            [SseEvent::Error(
                "The model's safety guardrails were triggered.".into()
            )]
        );
    }
}
