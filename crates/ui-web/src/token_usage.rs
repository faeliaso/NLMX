//! Token usage of the latest answer in the status bar, in the shape of `fm chat`:
//! `system · turn 1 · ↑ 101 · ↓ 19 · 120 / 8.192 (1% used)`. The counts come from the model
//! (session diagnostics), never from a constant; only the window size is fixed
//! ([`MODEL_CONTEXT_WINDOW`]).

use askama::Template;
use axum::{
    extract::{Query, State},
    response::{Html, IntoResponse, Response},
};
use http::{StatusCode, header::CACHE_CONTROL};
use nlmx_domain::generation::{LastGeneration, MODEL_CONTEXT_WINDOW};
use nlmx_i18n::{Locale, format_integer, format_percent, tr, tr_args};
use serde::Deserialize;

use crate::AppState;

/// What the indicator shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageState {
    /// No answer has been counted in this session.
    Idle,
    /// An answer is being generated: its counts are not known yet.
    Calculating,
    Counted(LastGeneration),
}

impl UsageState {
    fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Calculating => "calculating",
            Self::Counted(_) => "counted",
        }
    }
}

/// Display data of the indicator: compact text and the tooltip.
pub struct UsageView {
    pub state: &'static str,
    pub text: String,
    pub title: String,
}

fn count(locale: Locale, tokens: impl Into<u64>) -> String {
    format_integer(locale, tokens.into())
}

impl UsageView {
    pub fn new(locale: Locale, state: UsageState) -> Self {
        let heading = tr(locale, "status-usage-title");
        let scope = tr(locale, "status-usage-scope");
        let (text, detail) = match state {
            UsageState::Idle => ("—".to_string(), tr(locale, "status-usage-none")),
            UsageState::Calculating => (
                tr(locale, "status-usage-calculating"),
                tr(locale, "status-usage-calculating-detail"),
            ),
            UsageState::Counted(last) => {
                let usage = last.usage;
                let turn = last.turn.to_string();
                let prompt = count(locale, usage.prompt_tokens);
                let completion = count(locale, usage.completion_tokens);
                let total = count(locale, usage.total());
                let window = count(locale, MODEL_CONTEXT_WINDOW);
                let percent = format_percent(locale, usage.context_ratio());
                let line = tr_args(
                    locale,
                    "status-usage-line",
                    &[
                        ("turn", turn.as_str().into()),
                        ("prompt", prompt.as_str().into()),
                        ("completion", completion.as_str().into()),
                        ("total", total.as_str().into()),
                        ("window", window.as_str().into()),
                        ("percent", percent.as_str().into()),
                    ],
                );
                (line, tr(locale, "status-usage-detail"))
            }
        };
        Self {
            state: state.name(),
            text,
            title: format!("{heading}\n{detail}\n\n{scope}"),
        }
    }
}

#[derive(Template)]
#[template(path = "components/token_usage.html")]
struct UsageFragment {
    view: UsageView,
}

#[derive(Deserialize, Default)]
pub struct UsageQuery {
    /// Set by the page while an answer is being generated.
    busy: Option<u8>,
}

pub async fn fragment(State(state): State<AppState>, Query(query): Query<UsageQuery>) -> Response {
    let usage = if query.busy == Some(1) {
        UsageState::Calculating
    } else {
        state
            .diagnostics
            .as_ref()
            .and_then(|d| d.snapshot().last_generation)
            .map_or(UsageState::Idle, UsageState::Counted)
    };
    let fragment = UsageFragment {
        view: UsageView::new(nlmx_i18n::current(), usage),
    };
    match fragment.render() {
        // Changes after every answer: the page must never reuse an earlier response.
        Ok(html) => ([(CACHE_CONTROL, "no-store")], Html(html)).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use nlmx_domain::generation::GenerationUsage;

    use super::*;

    fn counted(turn: u32, prompt_tokens: u32, completion_tokens: u32) -> UsageState {
        UsageState::Counted(LastGeneration {
            turn,
            usage: GenerationUsage {
                prompt_tokens,
                completion_tokens,
            },
        })
    }

    #[test]
    fn idle_shows_a_dash_and_explains_itself() {
        let v = UsageView::new(Locale::PtBr, UsageState::Idle);
        assert_eq!((v.state, v.text.as_str()), ("idle", "—"));
        assert!(v.title.starts_with("Uso de tokens da última resposta"));
    }

    #[test]
    fn calculating_shows_no_numbers() {
        let v = UsageView::new(Locale::PtBr, UsageState::Calculating);
        assert_eq!((v.state, v.text.as_str()), ("calculating", "Calculando…"));
        assert!(!v.text.chars().any(|c| c.is_ascii_digit()));
    }

    #[test]
    fn the_line_matches_fm_chat() {
        let line = |locale, state| UsageView::new(locale, state).text;
        assert_eq!(
            line(Locale::En, counted(1, 101, 19)),
            "system · turn 1 · ↑ 101 · ↓ 19 · 120 / 8,192 (1% used)"
        );
        assert_eq!(
            line(Locale::PtBr, counted(1, 101, 19)),
            "system · turno 1 · ↑ 101 · ↓ 19 · 120 / 8.192 (1% usado)"
        );
        assert_eq!(
            line(Locale::PtBr, counted(7, 1655, 343)),
            "system · turno 7 · ↑ 1.655 · ↓ 343 · 1.998 / 8.192 (24% usado)"
        );
        assert_eq!(
            line(Locale::Es, counted(2, 300, 50)),
            "system · turno 2 · ↑ 300 · ↓ 50 · 350 / 8.192 (4 % usado)"
        );
    }

    #[test]
    fn the_percentage_can_pass_the_window_without_breaking() {
        let v = UsageView::new(Locale::En, counted(9, u32::MAX, u32::MAX));
        assert!(v.text.contains("↑ 4,294,967,295"), "{}", v.text);
        assert!(v.text.contains("% used"), "{}", v.text);
        let empty = UsageView::new(Locale::En, counted(1, 0, 0));
        assert!(
            empty.text.ends_with("0 / 8,192 (0% used)"),
            "{}",
            empty.text
        );
    }

    #[test]
    fn the_tooltip_explains_the_arrows_and_the_scope() {
        let v = UsageView::new(Locale::En, counted(1, 101, 19));
        assert_eq!(
            v.title,
            "Token usage of the last answer\n↑ input: instructions, conversation and question · ↓ output: generated answer\n\nCounted by the model on this Mac · last answer"
        );
    }
}
