//! Explicit, bounded transcript pages. Message bodies are read only for the
//! selected session and never stored in ingestion checkpoints or snapshots.
use super::*;
use crate::l10n;
#[derive(Default)]
pub(super) struct Context {
    path: PathBuf,
    cursor: u64,
    turn_id: Option<String>,
    started: Option<Value>,
}
impl Context {
    fn read(&mut self, value: &Value) {
        let payload = &value["payload"];
        if let Some(id) = payload["turn_id"].as_str() {
            if self.turn_id.as_deref() != Some(id) {
                self.started = None;
            }
            self.turn_id = Some(id.into());
            if value["type"] == "event_msg" && payload["type"] == "task_started" {
                self.started = value.get("timestamp").cloned();
            }
        } else if live_facts::claude_prompt(value) {
            if !claude_interrupted(value) {
                self.turn_id = live_facts::claude_turn(value)
                    .or_else(|| value["id"].as_str())
                    .map(str::to_owned);
                self.started = value
                    .get("timestamp")
                    .or_else(|| value.get("time"))
                    .cloned();
            }
        } else if value["type"] == "turn/start" {
            if let Some(turn) = value["data"]["turn"]
                .as_i64()
                .map(|n| n.to_string())
                .or_else(|| value["data"]["turn"].as_str().map(str::to_owned))
            {
                self.turn_id = Some(turn);
                self.started = value
                    .get("time")
                    .or_else(|| value.get("timestamp"))
                    .cloned();
            }
        } else if matches!(
            value["type"].as_str(),
            Some("user.message" | "assistant.turn_start")
        ) {
            if let Some(id) = value["id"].as_str() {
                self.turn_id = Some(id.to_owned());
                self.started = value.get("timestamp").cloned();
            }
        } else if value["type"] == "context.append_loop_event"
            && value["event"]["type"] == "step.begin"
        {
            if let Some(id) = value["event"]["turnId"].as_str() {
                self.turn_id = Some(id.to_owned());
                self.started = value.get("time").cloned();
            }
        }
    }
}
/// Claude Code's note that the user stopped a turn: a user message that starts
/// no turn of its own and is not the user's words.
fn claude_interrupted(value: &Value) -> bool {
    let content = &value["message"]["content"];
    content
        .as_str()
        .is_some_and(|s| s.starts_with("[Request interrupted"))
        || content.as_array().is_some_and(|parts| {
            parts.iter().any(|part| {
                part["text"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("[Request interrupted"))
            })
        })
}
fn text_value(value: Value) -> Value {
    match value {
        Value::Null => Value::Null,
        Value::String(text) => Value::String(text),
        Value::Array(items) => {
            let parts = items
                .into_iter()
                .filter_map(|item| match item {
                    Value::Null => None,
                    Value::String(text) => (!text.is_empty()).then_some(text),
                    Value::Object(mut map) => {
                        if let Some(text) = map.get("text").and_then(Value::as_str) {
                            return (!text.is_empty()).then(|| text.to_owned());
                        }
                        if let Some(content) = map.remove("content") {
                            if let Value::String(text) = text_value(content) {
                                return (!text.is_empty()).then_some(text);
                            }
                        }
                        if map.get("type").and_then(Value::as_str) == Some("image") {
                            return Some("[image]".to_owned());
                        }
                        if let Some(name) = map.get("tool_name").and_then(Value::as_str) {
                            return Some(format!("[tool: {name}]"));
                        }
                        serde_json::to_string(&Value::Object(map)).ok()
                    }
                    other => serde_json::to_string(&other).ok(),
                })
                .collect::<Vec<_>>();
            if parts.is_empty() {
                Value::Null
            } else {
                Value::String(parts.join("\n"))
            }
        }
        other => serde_json::to_string(&other)
            .map(Value::String)
            .unwrap_or(Value::Null),
    }
}
fn message(value: &Value, offset: u64) -> Vec<Value> {
    if live_facts::request_copy(value) {
        return Vec::new();
    }
    let timestamp = value
        .get("timestamp")
        .or_else(|| value.get("time"))
        .and_then(|v| {
            if let Some(s) = v.as_str() {
                Some(Value::String(s.to_owned()))
            } else if let Some(ms) = v.as_i64() {
                chrono::DateTime::from_timestamp_millis(ms).map(|dt| Value::String(dt.to_rfc3339()))
            } else {
                None
            }
        })
        .unwrap_or(Value::Null);
    let mut rows = Vec::new();
    let mut push = |id: String,
                    parent: Value,
                    role: &str,
                    text: Value,
                    tool: Value,
                    call: Value| {
        let text = text_value(text);
        rows.push(json!({"id":id,"parentId":parent,"role":role,"text":text,"timestamp":timestamp,"toolCallId":call,"toolCall":tool}));
    };
    if value["type"] == "response_item" {
        let payload = &value["payload"];
        let id = payload["id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("o{offset}"));
        match payload["type"].as_str() {
            Some("message") => {
                let text = payload["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|v| matches!(v["type"].as_str(), Some("input_text" | "output_text")))
                    .filter_map(|v| v["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    push(
                        id,
                        Value::Null,
                        payload["role"].as_str().unwrap_or("assistant"),
                        json!(text),
                        Value::Null,
                        Value::Null,
                    );
                }
            }
            Some("function_call" | "custom_tool_call") => push(
                id,
                Value::Null,
                "assistant",
                Value::Null,
                json!({"id":payload["call_id"],"name":payload["name"],"arguments":payload.get("arguments").or_else(||payload.get("input"))}),
                payload["call_id"].clone(),
            ),
            Some("function_call_output" | "custom_tool_call_output") => push(
                id,
                Value::Null,
                "tool",
                payload["output"].clone(),
                Value::Null,
                payload["call_id"].clone(),
            ),
            _ => {}
        }
    } else if matches!(
        value["type"].as_str(),
        Some("user/message" | "assistant/message")
    ) {
        let role = if value["type"] == "user/message" {
            "user"
        } else {
            "assistant"
        };
        let content = &value["data"]["content"];
        let text = content
            .as_str()
            .map(str::to_owned)
            .or_else(|| {
                content.as_array().map(|parts| {
                    parts
                        .iter()
                        .filter_map(|v| v["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            })
            .unwrap_or_default();
        if !text.is_empty() {
            push(
                format!("o{offset}"),
                Value::Null,
                role,
                json!(text),
                Value::Null,
                Value::Null,
            );
        }
    } else if matches!(
        value["type"].as_str(),
        Some("user.message" | "assistant.message")
    ) {
        let role = if value["type"] == "user.message" {
            "user"
        } else {
            "assistant"
        };
        let data = &value["data"];
        if let Some(text) = data["content"]
            .as_str()
            .or_else(|| data["text"].as_str())
            .filter(|s| !s.is_empty())
        {
            let id = value["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("o{offset}"));
            push(id, Value::Null, role, json!(text), Value::Null, Value::Null);
        }
    } else {
        let item = if value["message"].is_object() {
            &value["message"]
        } else if matches!(
            value["type"].as_str(),
            Some("user" | "assistant" | "message")
        ) {
            value
        } else {
            return rows;
        };
        let role = item["role"]
            .as_str()
            .or_else(|| value["role"].as_str())
            .or_else(|| value["type"].as_str())
            .unwrap_or("");
        if !matches!(role, "user" | "assistant" | "tool" | "toolResult") {
            return rows;
        }
        let id = value["uuid"]
            .as_str()
            .or_else(|| item["id"].as_str())
            .or_else(|| value["id"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("o{offset}"));
        let parent = value
            .get("parentUuid")
            .or_else(|| value.get("parentId"))
            .cloned()
            .unwrap_or(Value::Null);
        if let Some(text) = item["content"].as_str() {
            push(
                id,
                parent,
                role,
                json!(text),
                Value::Null,
                item["toolCallId"].clone(),
            );
        } else if let Some(blocks) = item["content"].as_array() {
            for (index, block) in blocks.iter().enumerate() {
                let key = format!("{id}:{index}");
                match block["type"].as_str() {
                    Some("text" | "input_text" | "output_text") => push(
                        key,
                        parent.clone(),
                        role,
                        block["text"].clone(),
                        Value::Null,
                        Value::Null,
                    ),
                    Some("tool_use" | "toolCall") => push(
                        key,
                        parent.clone(),
                        "assistant",
                        Value::Null,
                        json!({"id":block["id"],"name":block["name"],"arguments":block.get("input").or_else(||block.get("arguments"))}),
                        block["id"].clone(),
                    ),
                    Some("tool_result") => push(
                        key,
                        parent.clone(),
                        "tool",
                        block["content"].clone(),
                        Value::Null,
                        block["tool_use_id"].clone(),
                    ),
                    _ => {}
                }
            }
        } else if let Some(parts) = item["parts"].as_array() {
            let text = parts
                .iter()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if !text.is_empty() {
                push(id, parent, role, json!(text), Value::Null, Value::Null);
            }
        }
    }
    rows
}
pub(super) fn page(
    path: &Path,
    source: &str,
    cursor: u64,
    limit: usize,
    tail: bool,
    archives: &mut compressed::Archives,
    context: &mut Context,
) -> Result<Value, String> {
    if source_db::accepts(path, source) {
        return Err(l10n::text("此来源的压缩日志不支持分页查看消息；用量和调用仍可查看", "Message paging is not available for this compact source; usage and calls remain available"));
    }
    let metadata = fs::metadata(path).map_err(|_| {
        l10n::text(
            "所选会话的日志不可用",
            "The selected session transcript is unavailable",
        )
    })?;
    let stamp = FileStamp {
        source: "conversation",
        length: metadata.len(),
        modified: metadata.modified().ok(),
    };
    let mut budget = 2 * 1024 * 1024u64;
    let archive = compressed::is_archive(path);
    if tail && archive {
        return Err("Bounded tail pages require a plain transcript; compressed history has no random decoded seek".into());
    }
    let cursor = if tail {
        metadata.len().saturating_sub(2 * 1024 * 1024)
    } else {
        cursor
    };
    if tail || context.path != path || context.cursor != cursor {
        *context = Context {
            path: path.into(),
            cursor,
            turn_id: None,
            started: None,
        };
    }
    if archive
        && !archives
            .prepare(path, stamp, cursor, &mut budget)
            .map_err(|_| {
                l10n::text(
                    "无法解码所选会话的日志",
                    "Could not decode the selected session transcript",
                )
            })?
    {
        return Ok(
            json!({"messages":[],"messageCursor":cursor,"nextMessageCursor":cursor,"messagesPending":true,"messagesComplete":false}),
        );
    }
    let mut file;
    if archive {
        file = None;
    } else {
        let mut opened = File::open(path).map_err(|_| {
            l10n::text(
                "无法读取所选会话的日志",
                "Could not read the selected session transcript",
            )
        })?;
        opened
            .seek(SeekFrom::Start(cursor))
            .map_err(|_| "Invalid transcript cursor")?;
        file = Some(BufReader::new(opened));
    }
    let reader: &mut dyn BufRead = if archive {
        archives.reader()
    } else {
        file.as_mut().unwrap()
    };
    let mut position = cursor;
    if tail && cursor > 0 {
        let (_, consumed, _) =
            line(reader).map_err(|_| "Could not read transcript tail boundary")?;
        position += consumed;
        budget = budget.saturating_sub(consumed);
    }
    let mut messages = Vec::new();
    let mut complete = false;
    while budget > 0 && (tail || messages.len() < limit) {
        let (bytes, count, ended) = line(reader).map_err(|_| {
            l10n::text(
                "无法读取所选会话的日志",
                "Could not read the selected session transcript",
            )
        })?;
        if count == 0 || !ended {
            complete = true;
            break;
        }
        let start = position;
        position += count;
        budget = budget.saturating_sub(count);
        if bytes.len() > MAX_LINE {
            continue;
        }
        if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
            context.read(&value);
            let mut entries = message(&value, start);
            for entry in &mut entries {
                entry["turnId"] = json!(context.turn_id);
                entry["turnStartedAt"] = json!(context.started);
                // The user's own words: neither the context a client hands the
                // model nor its note of an interruption.
                entry["isPrompt"] = json!(
                    entry["role"] == "user"
                        && if value["message"].is_object() {
                            live_facts::claude_prompt(&value) && !claude_interrupted(&value)
                        } else {
                            agent_hud_agents::codex::transcript::user_words(&value)
                        }
                );
            }
            messages.extend(entries);
            if tail && messages.len() > limit {
                messages.drain(..messages.len() - limit);
            }
        }
    }
    context.cursor = position;
    if archive {
        archives.position(position);
        if complete {
            archives.forget(path);
        }
    }
    Ok(
        json!({"messages":messages,"messageCursor":cursor,"nextMessageCursor":(tail||!complete).then_some(position),"endMessageCursor":position,"messagesTruncatedBefore":tail&&cursor>0,"messagesPending":false,"messagesComplete":complete}),
    )
}
