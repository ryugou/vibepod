#[derive(Debug)]
pub enum StreamEvent {
    Display(String),
    Result(String),
    Skip,
    PassThrough(String),
}

/// Claude Code の stream-json `result` イベントから、呼び出し元向け要約に
/// 必要なフィールドだけを抽出した結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultSummary {
    /// `is_error` フィールド（欠落時は false 扱い）。
    pub is_error: bool,
    /// `subtype`（例: `"success"` / `"error_max_turns"` /
    /// `"error_during_execution"`）。欠落時は `None`。
    pub subtype: Option<String>,
    /// エージェントの最終メッセージ（`result` フィールド）。欠落時は `None`。
    pub result_text: Option<String>,
}

/// stream-json の 1 行を `result` イベントとして解釈し、要約に必要な
/// フィールドを取り出す純関数。
///
/// - `line` が `None`、JSON でない、または `type != "result"` の場合は
///   `None` を返す（＝要約すべき結果イベントが無かった）。
/// - I/O や時刻に依存しないためユニットテストで網羅できる。
pub fn summarize_result_line(line: Option<&str>) -> Option<ResultSummary> {
    let line = line?;
    let json: serde_json::Value = serde_json::from_str(line).ok()?;
    if json.get("type").and_then(|v| v.as_str()) != Some("result") {
        return None;
    }
    let is_error = json
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let subtype = json
        .get("subtype")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let result_text = json
        .get("result")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Some(ResultSummary {
        is_error,
        subtype,
        result_text,
    })
}

pub fn format_stream_event(line: &str) -> StreamEvent {
    match serde_json::from_str::<serde_json::Value>(line) {
        Ok(json) => {
            let event_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match event_type {
                "assistant" => {
                    if let Some(contents) = json
                        .get("message")
                        .and_then(|m| m.get("content"))
                        .and_then(|c| c.as_array())
                    {
                        let mut lines: Vec<String> = Vec::new();
                        for item in contents {
                            match item.get("type").and_then(|t| t.as_str()) {
                                Some("text") => {
                                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                        lines.push(format!("  │  [assistant] {}", text));
                                    }
                                }
                                Some("tool_use") => {
                                    let name = item
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("unknown");
                                    let input = item
                                        .get("input")
                                        .cloned()
                                        .unwrap_or(serde_json::Value::Null);
                                    let input_display = if let Some(obj) = input.as_object() {
                                        let pairs: Vec<String> = obj
                                            .iter()
                                            .map(|(k, v)| {
                                                let val = v
                                                    .as_str()
                                                    .map(|s| {
                                                        let mut truncated = String::new();
                                                        let mut count = 0usize;
                                                        let mut over_limit = false;
                                                        for ch in s.chars() {
                                                            if count < 77 {
                                                                truncated.push(ch);
                                                            }
                                                            count += 1;
                                                            if count > 80 {
                                                                over_limit = true;
                                                                break;
                                                            }
                                                        }
                                                        if over_limit {
                                                            format!("\"{}...\"", truncated)
                                                        } else {
                                                            format!("\"{}\"", s)
                                                        }
                                                    })
                                                    .unwrap_or_else(|| v.to_string());
                                                format!("{}: {}", k, val)
                                            })
                                            .collect();
                                        format!("{{ {} }}", pairs.join(", "))
                                    } else {
                                        input.to_string()
                                    };
                                    lines.push(format!(
                                        "  │  [tool_use] {} {}",
                                        name, input_display
                                    ));
                                }
                                _ => {}
                            }
                        }
                        if !lines.is_empty() {
                            return StreamEvent::Display(lines.join("\n"));
                        }
                    }
                    StreamEvent::Skip
                }
                "result" => {
                    if let Some(result_val) = json.get("result").and_then(|r| r.as_str()) {
                        StreamEvent::Result(result_val.to_string())
                    } else {
                        StreamEvent::Skip
                    }
                }
                "rate_limit_event" => {
                    if let Some(info) = json.get("rate_limit_info") {
                        let status = info.get("status").and_then(|s| s.as_str()).unwrap_or("");
                        if status != "allowed" {
                            let resets_at =
                                info.get("resetsAt").and_then(|r| r.as_str()).unwrap_or("");
                            let limit_type = info
                                .get("rateLimitType")
                                .and_then(|t| t.as_str())
                                .unwrap_or("");
                            StreamEvent::Display(format!(
                                "  │  [rate_limit] status: {}, resets_at: {}, type: {}",
                                status, resets_at, limit_type
                            ))
                        } else {
                            StreamEvent::Skip
                        }
                    } else {
                        StreamEvent::Skip
                    }
                }
                _ => StreamEvent::Skip,
            }
        }
        Err(_) => StreamEvent::PassThrough(line.to_string()),
    }
}
