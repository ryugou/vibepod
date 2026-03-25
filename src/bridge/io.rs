// Bridge I/O: bollard attach, terminal raw mode, stdin/stdout transfer, SIGWINCH resize

use anyhow::Result;
use bollard::container::AttachContainerResults;
use futures_util::StreamExt;
use std::os::unix::io::AsRawFd;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

/// ターミナルを raw mode に設定し、Drop で自動復元する RAII ガード。
/// panic hook も設定して、パニック時にもターミナルを復元する。
pub struct TerminalGuard {
    original_termios: libc::termios,
    fd: i32,
}

impl TerminalGuard {
    pub fn new() -> Result<Self> {
        let fd = std::io::stdin().as_raw_fd();
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut termios) } != 0 {
            anyhow::bail!("Failed to get terminal attributes");
        }
        let original = termios;

        // raw mode 設定
        unsafe { libc::cfmakeraw(&mut termios) };
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) } != 0 {
            anyhow::bail!("Failed to set terminal to raw mode");
        }

        // panic hook でターミナル復元
        let restore_termios = original;
        let old_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            unsafe { libc::tcsetattr(fd, libc::TCSANOW, &restore_termios) };
            old_hook(info);
        }));

        Ok(Self {
            original_termios: original,
            fd,
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.original_termios) };
    }
}

/// ホスト側ターミナルの現在のウィンドウサイズを取得する。
pub fn terminal_size() -> Option<(u16, u16)> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let fd = std::io::stdout().as_raw_fd();
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } == 0 {
        Some((ws.ws_col, ws.ws_row))
    } else {
        None
    }
}

/// メインの I/O ループ。コンテナの attach ストリームとターミナルの間をブリッジする。
///
/// - attach stdout → ターミナル stdout + output_tx（detector 向け）
/// - ターミナル stdin → attach stdin
/// - stdin_rx（Slack 応答）→ attach stdin
/// - SIGWINCH → resize_container_tty
/// - コンテナ終了で終了
pub async fn run_io_loop(
    attach_result: AttachContainerResults,
    output_tx: mpsc::Sender<Vec<u8>>,
    mut stdin_rx: mpsc::Receiver<Vec<u8>>,
    terminal_input_tx: mpsc::Sender<()>,
    runtime: &crate::runtime::DockerRuntime,
    container_id: &str,
) -> Result<()> {
    let AttachContainerResults { mut output, input } = attach_result;
    let mut container_stdin = input;

    let mut stdout = tokio::io::stdout();
    let mut stdin = tokio::io::stdin();
    let mut stdin_buf = [0u8; 4096];

    // SIGWINCH シグナル監視（Unix のみ）
    let mut sigwinch =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())?;

    // 初期ウィンドウサイズを同期
    if let Some((w, h)) = terminal_size() {
        runtime
            .resize_container_tty(container_id, w, h)
            .await
            .ok();
    }

    loop {
        tokio::select! {
            // コンテナ stdout → ターミナル stdout + output_tx
            chunk = output.next() => {
                match chunk {
                    Some(Ok(log_output)) => {
                        let bytes = log_output.into_bytes();
                        stdout.write_all(&bytes).await?;
                        stdout.flush().await?;
                        let vec: Vec<u8> = bytes.to_vec();
                        output_tx.send(vec).await.ok();
                    }
                    Some(Err(e)) => {
                        // ストリームエラー — コンテナが終了した可能性
                        eprintln!("attach output error: {}", e);
                        break;
                    }
                    None => {
                        // ストリーム終了 — コンテナが終了
                        break;
                    }
                }
            }

            // ターミナル stdin → コンテナ stdin
            n = stdin.read(&mut stdin_buf) => {
                match n {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        container_stdin.write_all(&stdin_buf[..n]).await?;
                        container_stdin.flush().await?;
                        terminal_input_tx.send(()).await.ok();
                    }
                    Err(e) => {
                        eprintln!("stdin read error: {}", e);
                        break;
                    }
                }
            }

            // Slack 応答 → コンテナ stdin
            Some(data) = stdin_rx.recv() => {
                container_stdin.write_all(&data).await?;
                container_stdin.flush().await?;
            }

            // SIGWINCH → コンテナ TTY リサイズ
            _ = sigwinch.recv() => {
                if let Some((w, h)) = terminal_size() {
                    runtime.resize_container_tty(container_id, w, h).await.ok();
                }
            }
        }
    }

    Ok(())
}
