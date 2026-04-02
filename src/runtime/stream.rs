#[derive(Debug)]
pub enum StreamEvent {
    Display(String),
    Result(String),
    Skip,
    PassThrough(String),
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
