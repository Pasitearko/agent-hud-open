//! Explicit, bounded transcript pages. Message bodies are read only for the
//! selected session and never stored in ingestion checkpoints or snapshots.
use super::*;
use crate::l10n;
use agent_hud_agents::transcript::turns::{self, TurnStart, Turns};
/// The turn a page's messages belong to where the page begins: the turn read
/// last before `cursor` in `path`. Which lines name a turn is the client's
/// own rule (`Agent::turns`); keeping and finding it is the same for all.
#[derive(Default)]
pub(super) struct Context {
    path: PathBuf,
    cursor: u64,
    turn_id: Option<String>,
    started: Option<Value>,
}
impl Context {
    fn read(&mut self, turns: &Turns, value: &Value) {
        let Some(start) = (turns.start)(value) else {
            return;
        };
        if start.whole {
            self.turn_id = start.turn;
            self.started = start.started;
            return;
        }
        if self.turn_id != start.turn {
            self.started = None;
        }
        self.turn_id = start.turn;
        if start.started.is_some() {
            self.started = start.started;
        }
    }
    /// The turn the line at `end` belongs to, read backwards from there: the
    /// nearest line that names a turn and, where that line does not start
    /// it, the line of the same turn that says when it began. Only lines the
    /// client says may name a turn are decoded; a turn that began more than
    /// `RECOVER_LIMIT` back stays unnamed.
    fn recover(&mut self, turns: &Turns, path: &Path, end: u64) {
        let Ok(mut file) = File::open(path) else {
            return;
        };
        let mut found: Option<TurnStart> = None;
        let mut settle = |line: &[u8]| -> bool {
            if line.is_empty() || line.len() > MAX_LINE || !(turns.may_name)(line) {
                return false;
            }
            let Ok(value) = serde_json::from_slice::<Value>(line) else {
                return false;
            };
            let Some(start) = (turns.start)(&value) else {
                return false;
            };
            match &mut found {
                None => {
                    let settled = start.whole || start.started.is_some();
                    found = Some(start);
                    settled
                }
                Some(seen) if !start.whole && start.turn == seen.turn => {
                    seen.started = start.started;
                    seen.started.is_some()
                }
                Some(_) => true,
            }
        };
        // `rest` is the end of the line whose start is not read yet; a line
        // past MAX_LINE is skipped, as pages skip it.
        let mut rest = Vec::new();
        let mut oversized = false;
        let mut position = end;
        let mut done = false;
        while position > 0 && end - position < RECOVER_LIMIT && !done {
            let size = position.min(RECOVER_CHUNK);
            position -= size;
            let mut chunk = vec![0; size as usize];
            if file.seek(SeekFrom::Start(position)).is_err()
                || std::io::Read::read_exact(&mut file, &mut chunk).is_err()
            {
                return;
            }
            let mut stop = chunk.len();
            while let Some(newline) = chunk[..stop].iter().rposition(|b| *b == b'\n') {
                if !oversized {
                    let mut line = chunk[newline + 1..stop].to_vec();
                    line.extend_from_slice(&rest);
                    if settle(&line) {
                        done = true;
                        break;
                    }
                }
                oversized = false;
                rest.clear();
                stop = newline;
            }
            if done {
                break;
            }
            if oversized || stop + rest.len() > MAX_LINE {
                oversized = true;
                rest.clear();
            } else {
                let mut line = chunk[..stop].to_vec();
                line.extend_from_slice(&rest);
                rest = line;
            }
        }
        if !done && position == 0 && !oversized {
            settle(&rest);
        }
        if let Some(found) = found {
            self.turn_id = found.turn;
            self.started = found.started;
        }
    }
}
/// How far back a page looks for the turn it begins in, and in what steps.
const RECOVER_LIMIT: u64 = 128 * 1024 * 1024;
const RECOVER_CHUNK: u64 = 1024 * 1024;
/// Each transcript's turn where its last page ended, so that pages of other
/// sessions read in between leave it, and a page that goes on where the last
/// one ended needs no look back.
#[derive(Default)]
pub(super) struct Contexts {
    by_path: HashMap<PathBuf, (u64, Context)>,
    uses: u64,
}
impl Contexts {
    const KEPT: usize = 256;
    fn take(&mut self, path: &Path, cursor: u64) -> Option<Context> {
        match self.by_path.get(path) {
            Some((_, context)) if context.cursor == cursor => {
                self.by_path.remove(path).map(|(_, context)| context)
            }
            _ => None,
        }
    }
    fn keep(&mut self, context: Context) {
        self.uses += 1;
        self.by_path
            .insert(context.path.clone(), (self.uses, context));
        if self.by_path.len() > Self::KEPT {
            if let Some(oldest) = self
                .by_path
                .iter()
                .min_by_key(|(_, (used, _))| *used)
                .map(|(path, _)| path.clone())
            {
                self.by_path.remove(&oldest);
            }
        }
    }
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
    contexts: &mut Contexts,
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
    // A page that goes on where this transcript's last page ended keeps its
    // turn; any other page looks back for it once it knows its first line.
    let remembered = (!tail).then(|| contexts.take(path, cursor)).flatten();
    let look_back = remembered.is_none() && !archive;
    let rule = turns::of(source);
    let mut context = remembered.unwrap_or_else(|| Context {
        path: path.into(),
        cursor,
        turn_id: None,
        started: None,
    });
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
    if look_back && position > 0 {
        context.recover(rule, path, position);
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
            context.read(rule, &value);
            let mut entries = message(&value, start);
            for entry in &mut entries {
                entry["turnId"] = json!(context.turn_id);
                entry["turnStartedAt"] = json!(context.started);
                // The user's own words: neither the context a client hands the
                // model nor its note of an interruption.
                entry["isPrompt"] = json!(
                    entry["role"] == "user"
                        && if value["message"].is_object() {
                            live_facts::claude_prompt(&value)
                                && !agent_hud_agents::claude::transcript::interrupted(&value)
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
    contexts.keep(context);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(rows: &[Value], filler: usize) -> (tempfile::TempDir, PathBuf, u64) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("log.jsonl");
        let mut file = File::create(&path).unwrap();
        let mut end = 0;
        for (n, row) in rows.iter().enumerate() {
            let line = format!("{row}\n");
            file.write_all(line.as_bytes()).unwrap();
            end += line.len() as u64;
            if n == 0 {
                // Lines that name no turn, past one look-back step, and one
                // line longer than any page reads.
                for _ in 0..filler {
                    let line = format!("{}\n", json!({"type":"other","pad":"x".repeat(4096)}));
                    file.write_all(line.as_bytes()).unwrap();
                    end += line.len() as u64;
                }
                let long = format!(
                    "{}\n",
                    json!({"type":"user","pad":"y".repeat(MAX_LINE + 10)})
                );
                file.write_all(long.as_bytes()).unwrap();
                end += long.len() as u64;
            }
        }
        (temp, path, end)
    }

    fn recovered(source: &str, path: &Path, end: u64) -> (Option<String>, Option<Value>) {
        let mut context = Context::default();
        context.recover(turns::of(source), path, end);
        (context.turn_id, context.started)
    }

    #[test]
    fn looking_back_finds_a_claude_prompt_far_behind() {
        let (_temp, path, end) = write(
            &[
                json!({"type":"user","timestamp":"T0","uuid":"u0","promptId":"p1","message":{"role":"user","content":"Go"}}),
                json!({"type":"assistant","timestamp":"T1","message":{"role":"assistant","content":[{"type":"text","text":"Working."}]}}),
                json!({"type":"user","timestamp":"T2","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"x","content":"ok"}]}}),
            ],
            600,
        );
        assert_eq!(
            recovered("claude", &path, end),
            (Some("p1".into()), Some(json!("T0")))
        );
    }

    #[test]
    fn looking_back_finds_when_a_codex_turn_began() {
        let (_temp, path, end) = write(
            &[
                json!({"type":"event_msg","timestamp":"T0","payload":{"type":"task_started","turn_id":"t1"}}),
                json!({"type":"response_item","timestamp":"T1","payload":{"type":"message","turn_id":"t1"}}),
            ],
            600,
        );
        assert_eq!(
            recovered("codex", &path, end),
            (Some("t1".into()), Some(json!("T0")))
        );
    }

    #[test]
    fn looking_back_stops_at_the_turn_before() {
        let (_temp, path, end) = write(
            &[
                json!({"type":"event_msg","timestamp":"T0","payload":{"type":"task_started","turn_id":"t0"}}),
                json!({"type":"response_item","timestamp":"T1","payload":{"type":"message","turn_id":"t1"}}),
            ],
            0,
        );
        assert_eq!(recovered("codex", &path, end), (Some("t1".into()), None));
    }

    #[test]
    fn a_line_at_the_file_start_counts() {
        let (_temp, path, end) = write(
            &[
                json!({"type":"user","timestamp":"T0","uuid":"u0","promptId":"p1","message":{"role":"user","content":"Go"}}),
            ],
            0,
        );
        assert_eq!(
            recovered("claude", &path, end).0,
            Some("p1".into()),
            "the long line after it is skipped, the prompt still counts"
        );
        let only = path.with_file_name("only.jsonl");
        let row = format!(
            "{}\n",
            json!({"type":"user","timestamp":"T0","uuid":"u0","promptId":"p1","message":{"role":"user","content":"Go"}})
        );
        fs::write(&only, &row).unwrap();
        assert_eq!(
            recovered("claude", &only, row.len() as u64).0,
            Some("p1".into())
        );
    }
}
