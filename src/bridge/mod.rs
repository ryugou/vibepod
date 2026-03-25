pub mod detector;
pub mod formatter;
pub mod io;
pub mod logger;
pub mod slack;

use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use self::detector::{DetectorState, IdleDetector};
use self::formatter::{Formatter, LlmProvider};
use self::logger::BridgeLogger;
use self::slack::{SlackClient, SlackResponse};

pub struct BridgeConfig {
    pub slack_bot_token: String,
    pub slack_app_token: String,
    pub slack_channel_id: String,
    pub notify_delay_secs: u64,
    pub session_id: String,
    pub project_name: String,
    pub llm_provider: LlmProvider,
    pub llm_api_key: String,
}

pub async fn run(
    config: BridgeConfig,
    runtime: &crate::runtime::DockerRuntime,
    container_id: &str,
) -> Result<i64> {
    // 1. ログディレクトリ作成、BridgeLogger 初期化
    let config_dir = crate::config::default_config_dir()?;
    let log_path = config_dir
        .join("bridge-logs")
        .join(format!("{}.jsonl", config.session_id));
    let mut logger = BridgeLogger::new(&log_path)?;

    // 2. LLM Formatter 初期化
    let text_formatter = Formatter::new(config.llm_provider, config.llm_api_key);

    // 3. SlackClient 初期化・接続
    // notify_slack 用にトークンを先にクローン（slack は event_loop で move される）
    let bot_token = config.slack_bot_token;
    let app_token = config.slack_app_token;
    let channel_id = config.slack_channel_id;

    let mut slack = SlackClient::new(
        bot_token.clone(),
        app_token.clone(),
        channel_id.clone(),
        config.project_name.clone(),
        config.session_id.clone(),
    );

    // Slack 接続失敗時はターミナル専用モードにフォールバック
    let slack_active = Arc::new(AtomicBool::new(false));
    let slack_available = match slack.connect().await {
        Ok(_) => {
            // 3. セッション開始通知
            if let Err(e) = slack.notify_session_start().await {
                eprintln!("Warning: Failed to send session start notification: {}", e);
            }
            true
        }
        Err(e) => {
            eprintln!(
                "Warning: Slack connection failed, continuing in terminal-only mode: {}",
                e
            );
            false
        }
    };
    slack_active.store(slack_available, Ordering::SeqCst);

    // 4. TerminalGuard で raw mode 設定
    let _terminal_guard = io::TerminalGuard::new()
        .context("Failed to set terminal to raw mode")?;

    // 5. attach_container でストリーム取得
    let attach_result = runtime.attach_container(container_id).await?;

    // 6. mpsc channel 作成
    let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(256);
    let (slack_response_tx, mut slack_response_rx) = mpsc::channel::<SlackResponse>(16);
    let (stdin_tx, stdin_rx) = mpsc::channel::<Vec<u8>>(64);
    let (terminal_input_tx, mut terminal_input_rx) = mpsc::channel::<()>(64);

    // 7. tokio::spawn で I/O ループと Slack イベントループを起動
    let io_runtime = runtime.clone();
    let io_container_id = container_id.to_string();
    let mut io_handle = tokio::spawn(async move {
        io::run_io_loop(
            attach_result,
            output_tx,
            stdin_rx,
            terminal_input_tx,
            &io_runtime,
            &io_container_id,
        )
        .await
    });

    let slack_handle = if slack_available {
        let tx = slack_response_tx.clone();
        let slack_active_flag = slack_active.clone();
        Some(tokio::spawn(async move {
            if let Err(e) = slack.event_loop(tx).await {
                eprintln!("Warning: Slack event loop ended, Slack input disabled: {}", e);
                slack_active_flag.store(false, Ordering::SeqCst);
            }
        }))
    } else {
        None
    };

    // SlackClient for notifications (separate instance for main loop usage)
    let notify_slack = SlackClient::new(
        bot_token,
        app_token,
        channel_id,
        config.project_name.clone(),
        config.session_id.clone(),
    );

    // 8. メインループ: detector + Slack 応答処理
    let mut detector = IdleDetector::new(Duration::from_secs(config.notify_delay_secs));
    let slack_blocked = Arc::new(AtomicBool::new(false));
    let mut _current_message_ts: Option<String> = None;
    let mut notification_sent_at: Option<Instant> = None;
    let mut check_interval = tokio::time::interval(Duration::from_millis(500));

    loop {
        tokio::select! {
            // コンテナ出力を detector に渡す
            Some(data) = output_rx.recv() => {
                if detector.state() == DetectorState::WaitingResponse {
                    detector.on_output_resumed();
                    slack_blocked.store(false, Ordering::SeqCst);
                    _current_message_ts = None;
                    notification_sent_at = None;
                }
                detector.on_output(&data);
            }

            // ターミナル入力検知
            Some(()) = terminal_input_rx.recv() => {
                if detector.state() == DetectorState::WaitingResponse {
                    // ターミナル入力による応答 — ログ記録 + Slack のみブロック
                    let response_time = notification_sent_at
                        .map(|t| t.elapsed().as_secs())
                        .unwrap_or(0);
                    logger.log_responded("terminal", "(terminal input)", response_time).ok();
                    slack_blocked.store(true, Ordering::SeqCst);
                    detector.on_response();
                    _current_message_ts = None;
                    notification_sent_at = None;
                }
                detector.on_terminal_input();
            }

            // Slack 応答受信
            Some(response) = slack_response_rx.recv() => {
                if slack_blocked.load(Ordering::SeqCst) {
                    // ターミナル入力後の Slack 応答はブロック
                    continue;
                }

                if detector.state() == DetectorState::WaitingResponse {
                    // 応答を stdin に送信
                    let stdin_text = response.text.clone();
                    stdin_tx.send(stdin_text.into_bytes()).await.ok();

                    // 応答時間を計算してログ記録
                    let response_time = notification_sent_at
                        .map(|t| t.elapsed().as_secs())
                        .unwrap_or(0);
                    logger.log_responded(&response.source, &response.text, response_time).ok();

                    // Slack メッセージを応答済みに更新
                    if slack_active.load(Ordering::SeqCst) {
                        notify_slack
                            .update_responded(&response.message_ts, &response.text.trim_end())
                            .await
                            .ok();
                    }

                    detector.on_response();
                    slack_blocked.store(false, Ordering::SeqCst);
                    _current_message_ts = None;
                    notification_sent_at = None;
                }
            }

            // 定期的に無音検知チェック
            _ = check_interval.tick() => {
                if let Some(detector::DetectorEvent::Notify(raw_content)) = detector.check_idle() {
                    // LLM で TUI 出力を整形
                    let content = text_formatter.format(&raw_content).await;
                    if content.is_empty() {
                        // LLM が意味のあるコンテンツなしと判断 → スキップ
                        detector.on_response();
                        continue;
                    }

                    logger.log_notified(&content).ok();

                    if slack_active.load(Ordering::SeqCst) {
                        match notify_slack.notify_idle(&content).await {
                            Ok(ts) => {
                                _current_message_ts = Some(ts);
                                notification_sent_at = Some(Instant::now());
                                slack_blocked.store(false, Ordering::SeqCst);
                            }
                            Err(e) => {
                                eprintln!("Warning: Failed to send Slack notification: {}", e);
                                // 送信失敗時は Buffering に戻して再検知を可能にする
                                detector.on_response();
                            }
                        }
                    } else {
                        // Slack 未接続時も Buffering に戻す
                        detector.on_response();
                    }
                }
            }

            // I/O ループ終了 = コンテナ終了
            result = &mut io_handle => {
                if let Err(e) = result {
                    eprintln!("I/O loop error: {}", e);
                }
                break;
            }
        }
    }

    // 9. wait_container で exit code 取得
    let exit_code = runtime.wait_container(container_id).await.unwrap_or(0);

    // 10. セッション終了通知
    if slack_active.load(Ordering::SeqCst) {
        notify_slack.notify_session_end(exit_code).await.ok();
    }

    // Slack イベントループを停止
    if let Some(handle) = slack_handle {
        handle.abort();
    }

    // 11. TerminalGuard は drop で自動復元
    // 12. exit code を返却
    Ok(exit_code)
}
