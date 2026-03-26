// Slack client: Socket Mode connection, Block Kit notifications, response handling, reconnection

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;

/// Slack からの応答
pub struct SlackResponse {
    /// stdin に送信するテキスト
    pub text: String,
    /// 応答ソース: "slack_button", "slack_reaction", "slack_thread"
    pub source: String,
    /// 応答済み更新用のメッセージ ts
    pub message_ts: String,
}

/// Slack Socket Mode クライアント
pub struct SlackClient {
    bot_token: String,
    app_token: String,
    channel_id: String,
    project_name: String,
    session_id: String,
    http: reqwest::Client,
}

impl SlackClient {
    pub fn new(
        bot_token: String,
        app_token: String,
        channel_id: String,
        project_name: String,
        session_id: String,
    ) -> Self {
        Self {
            bot_token,
            app_token,
            channel_id,
            project_name,
            session_id,
            http: reqwest::Client::new(),
        }
    }

    /// Socket Mode WebSocket 接続を確立
    pub async fn connect(&mut self) -> Result<WebSocketConnection> {
        let resp: Value = self
            .http
            .post("https://slack.com/api/apps.connections.open")
            .bearer_auth(&self.app_token)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send()
            .await
            .context("Failed to request Socket Mode connection")?
            .json()
            .await?;

        if resp["ok"] != true {
            bail!(
                "Slack apps.connections.open failed: {}",
                resp["error"].as_str().unwrap_or("unknown error")
            );
        }

        let ws_url = resp["url"]
            .as_str()
            .context("No WebSocket URL in response")?;

        let (ws_stream, _) = tokio_tungstenite::connect_async(ws_url)
            .await
            .context("Failed to connect to Slack Socket Mode WebSocket")?;

        Ok(WebSocketConnection { ws: ws_stream })
    }

    /// セッション開始通知を送信
    pub async fn notify_session_start(&self) -> Result<()> {
        let text = format!(
            "\u{1f7e2} VibePod セッション開始 [{}] (session: {})",
            self.project_name, self.session_id
        );
        self.post_message(&text, None).await?;
        Ok(())
    }

    /// セッション終了通知を送信
    pub async fn notify_session_end(&self, exit_code: i64) -> Result<()> {
        let text = format!(
            "\u{1f534} VibePod セッション終了 [{}] (exit code: {})",
            self.project_name, exit_code
        );
        self.post_message(&text, None).await?;
        Ok(())
    }

    /// 無音検知通知を送信（Block Kit: コードブロック + Yes/No/Skip ボタン）
    /// 戻り値: メッセージの ts（更新用）
    pub async fn notify_idle(&self, buffer_content: &str, choices: &[String]) -> Result<String> {
        let blocks = build_idle_notification_blocks(
            buffer_content,
            &self.project_name,
            &self.session_id,
            choices,
        );

        let text = format!(
            "\u{1f514} VibePod [{}] (session: {}): セッション出力が停止しました",
            self.project_name, self.session_id
        );

        let body = json!({
            "channel": self.channel_id,
            "text": text,
            "blocks": blocks,
        });

        let resp: Value = self
            .http
            .post("https://slack.com/api/chat.postMessage")
            .bearer_auth(&self.bot_token)
            .json(&body)
            .send()
            .await
            .context("Failed to post idle notification")?
            .json()
            .await?;

        if resp["ok"] != true {
            bail!(
                "Slack chat.postMessage failed: {}",
                resp["error"].as_str().unwrap_or("unknown error")
            );
        }

        resp["ts"]
            .as_str()
            .map(|s| s.to_string())
            .context("No ts in chat.postMessage response")
    }

    /// 応答済みメッセージに更新（ボタン無効化）
    pub async fn update_responded(&self, message_ts: &str, response_text: &str) -> Result<()> {
        let body = json!({
            "channel": self.channel_id,
            "ts": message_ts,
            "text": format!("\u{2705} 応答済み: \"{}\"", response_text),
            "blocks": [],
        });

        let resp: Value = self
            .http
            .post("https://slack.com/api/chat.update")
            .bearer_auth(&self.bot_token)
            .json(&body)
            .send()
            .await
            .context("Failed to update message")?
            .json()
            .await?;

        if resp["ok"] != true {
            eprintln!(
                "Warning: Slack chat.update failed: {}",
                resp["error"].as_str().unwrap_or("unknown error")
            );
        }

        Ok(())
    }

    /// Socket Mode イベントループ。応答を受信したら response_tx に送信。
    /// 再接続: exponential backoff（初回1秒、最大60秒、最大5回）。
    /// 5回失敗で Err を返してフォールバック。
    pub async fn event_loop(&mut self, response_tx: mpsc::Sender<SlackResponse>) -> Result<()> {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let mut retry_count = 0u32;
        let mut backoff = Duration::from_secs(1);
        const MAX_RETRIES: u32 = 5;
        const MAX_BACKOFF: Duration = Duration::from_secs(60);

        loop {
            let ws_conn = match self.connect().await {
                Ok(conn) => {
                    retry_count = 0;
                    backoff = Duration::from_secs(1);
                    conn
                }
                Err(e) => {
                    retry_count += 1;
                    if retry_count > MAX_RETRIES {
                        bail!("Slack Socket Mode: failed to connect after {} retries: {}", MAX_RETRIES, e);
                    }
                    eprintln!(
                        "Slack Socket Mode: connection failed (attempt {}/{}), retrying in {:?}: {}",
                        retry_count, MAX_RETRIES, backoff, e
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            };

            let (mut write, mut read) = ws_conn.ws.split();

            loop {
                match read.next().await {
                    Some(Ok(Message::Text(text))) => {
                        let envelope: Value = match serde_json::from_str(&text) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

                        // Socket Mode: acknowledge envelope
                        if let Some(envelope_id) = envelope["envelope_id"].as_str() {
                            let ack = json!({"envelope_id": envelope_id});
                            if let Err(e) = write.send(Message::Text(ack.to_string().into())).await {
                                eprintln!("Failed to send envelope ack: {}", e);
                                break;
                            }
                        }

                        // Parse event type
                        let event_type = envelope["type"].as_str().unwrap_or("");

                        match event_type {
                            "interactive" => {
                                // Button press
                                if let Some(response) = self.handle_interaction(&envelope) {
                                    let _ = response_tx.send(response).await;
                                }
                            }
                            "events_api" => {
                                let event = &envelope["payload"]["event"];
                                let event_type = event["type"].as_str().unwrap_or("");

                                match event_type {
                                    "reaction_added" => {
                                        if let Some(response) = self.handle_reaction(event) {
                                            let _ = response_tx.send(response).await;
                                        }
                                    }
                                    "message" => {
                                        if let Some(response) = self.handle_message(event) {
                                            let _ = response_tx.send(response).await;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            "hello" => {
                                // Socket Mode handshake - connection established
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = write.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        // Connection closed, try to reconnect
                        break;
                    }
                    Some(Err(e)) => {
                        eprintln!("Slack WebSocket error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }

            // Connection lost, attempt reconnect with backoff
            retry_count += 1;
            if retry_count > MAX_RETRIES {
                bail!("Slack Socket Mode: lost connection after {} retries", MAX_RETRIES);
            }
            eprintln!(
                "Slack Socket Mode: connection lost, reconnecting (attempt {}/{}) in {:?}",
                retry_count, MAX_RETRIES, backoff
            );
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    fn handle_interaction(&self, envelope: &Value) -> Option<SlackResponse> {
        let payload = &envelope["payload"];
        let actions = payload["actions"].as_array()?;
        let action = actions.first()?;
        let action_id = action["action_id"].as_str()?;
        // message.ts identifies the notification message this button belongs to.
        // Used by update_responded() to replace the notification with "responded" status.
        let message_ts = payload["message"]["ts"].as_str()?;

        let text = map_action_to_stdin(action_id)?;

        Some(SlackResponse {
            text,
            source: "slack_button".to_string(),
            message_ts: message_ts.to_string(),
        })
    }

    fn handle_reaction(&self, event: &Value) -> Option<SlackResponse> {
        let reaction = event["reaction"].as_str()?;
        // item.ts is the message the reaction was added to.
        // This should match a notification we sent; unrelated reactions are harmless
        // (they produce a SlackResponse but update_responded will target our message).
        let message_ts = event["item"]["ts"].as_str()?;

        let text = map_reaction_to_stdin(reaction)?;

        Some(SlackResponse {
            text,
            source: "slack_reaction".to_string(),
            message_ts: message_ts.to_string(),
        })
    }

    fn handle_message(&self, event: &Value) -> Option<SlackResponse> {
        // Ignore bot's own messages
        if event.get("bot_id").is_some() {
            return None;
        }
        // Ignore message subtypes (edits, deletes, etc.)
        if event.get("subtype").is_some() {
            return None;
        }

        let user_text = event["text"].as_str()?;
        let thread_ts = event["thread_ts"].as_str();

        let (source, message_ts) = if let Some(ts) = thread_ts {
            // Thread reply: thread_ts is the parent notification message.
            // Used by update_responded() to mark the original notification as handled.
            ("slack_thread", ts.to_string())
        } else {
            // Direct message (not in a thread): ts is the message itself.
            // update_responded() targets this message — which is the DM, not a notification.
            // This is acceptable for DM-only flows.
            let ts = event["ts"].as_str().unwrap_or("").to_string();
            ("slack_dm", ts)
        };

        Some(SlackResponse {
            text: format!("{}\r", user_text),
            source: source.to_string(),
            message_ts,
        })
    }

    async fn post_message(&self, text: &str, blocks: Option<&Value>) -> Result<Value> {
        let mut body = json!({
            "channel": self.channel_id,
            "text": text,
        });
        if let Some(blocks) = blocks {
            body["blocks"] = blocks.clone();
        }

        let resp: Value = self
            .http
            .post("https://slack.com/api/chat.postMessage")
            .bearer_auth(&self.bot_token)
            .json(&body)
            .send()
            .await
            .context("Failed to post message")?
            .json()
            .await?;

        if resp["ok"] != true {
            bail!(
                "Slack chat.postMessage failed: {}",
                resp["error"].as_str().unwrap_or("unknown error")
            );
        }

        Ok(resp)
    }
}

/// WebSocket 接続のラッパー
pub struct WebSocketConnection {
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

/// Block Kit メッセージ構造を構築（無音検知通知用）
/// choices が空の場合はボタンなし、ありの場合は選択肢 + Skip ボタン
pub fn build_idle_notification_blocks(
    buffer_content: &str,
    project_name: &str,
    session_id: &str,
    choices: &[String],
) -> Value {
    let section = json!({
        "type": "section",
        "text": {
            "type": "mrkdwn",
            "text": format!(
                "\u{1f514} *VibePod* [{}] (session: `{}`)\nセッション出力が停止しました\n```\n{}\n```",
                project_name, session_id, buffer_content
            )
        }
    });

    let context = json!({
        "type": "context",
        "elements": [{
            "type": "mrkdwn",
            "text": "スレッドに返信でテキスト入力も可能"
        }]
    });

    if choices.is_empty() {
        return json!([section, context]);
    }

    // 選択肢ボタン生成
    let mut elements: Vec<Value> = choices
        .iter()
        .enumerate()
        .map(|(i, choice)| {
            let mut btn = json!({
                "type": "button",
                "text": { "type": "plain_text", "text": choice_display_text(choice) },
                "action_id": format!("respond_choice_{}", choice),
            });
            if i == 0 {
                btn["style"] = json!("primary");
            }
            if choice.to_lowercase() == "no" {
                btn["style"] = json!("danger");
            }
            btn
        })
        .collect();

    // Skip ボタンを末尾に追加
    elements.push(json!({
        "type": "button",
        "text": { "type": "plain_text", "text": "Skip" },
        "action_id": "respond_skip",
    }));

    let actions = json!({
        "type": "actions",
        "elements": elements,
    });

    json!([section, actions, context])
}

/// 選択肢の表示テキスト（yes → Yes, A → A）
fn choice_display_text(choice: &str) -> String {
    match choice {
        "yes" => "Yes".to_string(),
        "no" => "No".to_string(),
        _ => choice.to_string(),
    }
}

/// ボタン action_id → stdin テキストのマッピング
/// pty raw mode では Enter は \r（キャリッジリターン）
/// 動的ボタン: "respond_choice_X" → "X\r"
pub fn map_action_to_stdin(action_id: &str) -> Option<String> {
    if action_id == "respond_skip" {
        return Some("\r".to_string());
    }
    if let Some(choice) = action_id.strip_prefix("respond_choice_") {
        return Some(format!("{}\r", choice));
    }
    // レガシー互換（旧ボタンが残っている場合。Claude Code は y/n 単文字を期待）
    match action_id {
        "respond_yes" => Some("y\r".to_string()),
        "respond_no" => Some("n\r".to_string()),
        _ => None,
    }
}

/// リアクション名 → stdin テキストのマッピング
pub fn map_reaction_to_stdin(reaction: &str) -> Option<String> {
    match reaction {
        "+1" | "thumbsup" => Some("y\r".to_string()),
        "-1" | "thumbsdown" => Some("n\r".to_string()),
        "fast_forward" | "black_right_pointing_double_triangle_with_vertical_bar" => {
            Some("\r".to_string())
        }
        _ => None,
    }
}
