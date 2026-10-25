//! Turns the lines a collection pass reads into conversation facts, in the
//! same read that counts their tokens (`conversation_feed`). Which lines hold
//! which items is each client's own rule (`Agent::items`); the turn they
//! belong to is the parser's own turn tracking (`LiveLog::turns`), so a line
//! needs no second read and no look-back.
use crate::conversation_feed::{Fact, Item, Kind, Role};
use agent_hud_agents::transcript::{
    items::{self, LineItem, LineKind, LineRole},
    Log,
};
use serde_json::Value;

/// The log's turn just before a line is parsed.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Before {
    turn: Option<String>,
    running: bool,
}

pub(crate) fn before(log: &Log) -> Before {
    let turn = log.live.turns.back();
    Before {
        turn: turn.and_then(|turn| turn.id.clone()),
        running: turn.is_some_and(|turn| turn.running),
    }
}

/// What the line at `offset`, just parsed into `log`, adds to its
/// conversation.
pub(crate) fn line(log: &Log, value: &Value, offset: u64, before: &Before, facts: &mut Vec<Fact>) {
    let now = log.live.turns.back();
    let turn = now.and_then(|turn| turn.id.clone());
    let at_ms = timestamp(value);
    let lines = (items::of(&log.source).read)(value, offset);
    let starts = turn.is_some() && turn != before.turn;
    if starts {
        let first = lines.iter().find(|item| item.is_prompt);
        facts.push(Fact::TurnStarted {
            turn: turn.clone().unwrap_or_default(),
            prompt: first.and_then(|item| item.text.clone()),
            origin: first.and_then(|item| item.origin.clone()),
            at_ms: now.and_then(|turn| turn.started).or(at_ms),
        });
    }
    for (index, line) in lines.iter().enumerate() {
        if let Some(item) = item(line, &turn, offset, index, at_ms, starts) {
            facts.push(Fact::Item(item));
        }
    }
    let ended = now.is_some_and(|now| !now.running && now.id == before.turn && before.running);
    if let (true, Some(turn)) = (ended, turn) {
        facts.push(Fact::TurnEnded {
            turn,
            at_ms,
            outcome: now.and_then(|now| now.outcome.clone()),
        });
    }
}

/// The item a line's entry is: the user's words added to a running turn, or
/// the agent's text, tool call or thinking. A prompt that starts its turn
/// (`starts`) is that turn's start, and a tool's result is no item; a prompt
/// inside a running turn (a Codex steer) is the user's words in it.
pub(crate) fn item(
    line: &LineItem,
    turn: &Option<String>,
    offset: u64,
    index: usize,
    at_ms: Option<i64>,
    starts: bool,
) -> Option<Item> {
    if line.is_prompt && starts {
        return None;
    }
    let (role, kind, text) = match (&line.role, &line.kind) {
        // Only the user's own words: not the context or notes a client
        // writes as user lines.
        (LineRole::User, LineKind::Text) if line.is_prompt => {
            (Role::User, Kind::Text, line.text.clone()?)
        }
        (LineRole::Assistant, LineKind::Text) => (Role::Assistant, Kind::Text, line.text.clone()?),
        (LineRole::Assistant, LineKind::Tool) => (
            Role::Assistant,
            Kind::Tool,
            line.tool.as_ref().map(|tool| tool.name.clone())?,
        ),
        (LineRole::Assistant, LineKind::Thinking) => (
            Role::Assistant,
            Kind::Thinking,
            line.text.clone().unwrap_or_default(),
        ),
        _ => return None,
    };
    if kind == Kind::Text && text.trim().is_empty() {
        return None;
    }
    Some(Item {
        turn: turn.clone(),
        id: line.id.clone(),
        order: offset
            .saturating_mul(64)
            .saturating_add(index.min(63) as u64),
        role,
        kind,
        text,
        at_ms: line
            .timestamp
            .as_deref()
            .and_then(crate::metering::date_ms)
            .or(at_ms),
    })
}

fn timestamp(value: &Value) -> Option<i64> {
    let at = value.get("timestamp").or_else(|| value.get("time"))?;
    at.as_i64()
        .or_else(|| at.as_str().and_then(crate::metering::date_ms))
}
