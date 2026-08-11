use anyhow::{bail, Context, Result};

use crate::auth::{self, AuthManager, TokenData};
use crate::config;
use crate::runtime::DockerRuntime;

pub async fn execute() -> Result<()> {
    println!("\n  ┌  VibePod Login");
    println!("  │");

    // `vibepod login` は対話ストリームを 2 つ必要とする。stdin は
    // `auth::run_setup_token`（`src/auth.rs`）が `docker exec -it` で
    // OAuth フローを実行する際にホストの端末へ接続するために使い、
    // stderr は既存トークンの上書き確認 `dialoguer::Confirm`
    // （`Term::stderr()` を使う）が使う。どちらか一方でも非 TTY だと、
    // stdin 側は `docker exec -it` が `the input device is not a TTY` で
    // 落ち、stderr 側は確認プロンプトが出せない。両方を満たさない限り
    // このコマンドは完走できないため、コマンド全体の前提条件として
    // ここで 1 回だけ AND 条件で判定する。
    ensure_interactive_terminal(
        std::io::IsTerminal::is_terminal(&std::io::stdin()),
        std::io::IsTerminal::is_terminal(&std::io::stderr()),
    )?;

    let runtime = DockerRuntime::new()
        .await
        .context("Docker is not running. Please start Docker Desktop or OrbStack.")?;

    let config_dir = config::default_config_dir()?;
    let global_config = config::load_global_config(&config_dir)?;

    if !runtime.image_exists(&global_config.image).await? {
        bail!(
            "Docker image '{}' not found. Run `vibepod init` first.",
            global_config.image
        );
    }

    let auth_manager = AuthManager::new(config_dir.clone());

    if let Some(existing) = auth_manager.load_token()? {
        if !existing.is_expired() {
            println!(
                "  ⚠  Existing token found (valid until {}).",
                chrono::DateTime::parse_from_rfc3339(&existing.expires_at)
                    .map(|dt| dt.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|_| existing.expires_at.clone())
            );
            if !dialoguer::Confirm::new()
                .with_prompt("  Overwrite?")
                .default(false)
                .interact()?
            {
                println!("  └\n");
                return Ok(());
            }
        }
    }

    println!("  ◇  Creating long-lived token for container use...");
    println!("  │");

    let token = auth::run_setup_token(&global_config.image)?;

    let now = chrono::Utc::now();
    let expires_at = now + chrono::Duration::days(365);
    let token_data = TokenData {
        token,
        created_at: now.to_rfc3339(),
        expires_at: expires_at.to_rfc3339(),
    };
    auth_manager.save_token(&token_data)?;

    println!("  │");
    println!("  ◇  Login successful! Token saved.");
    println!("  │");
    println!("  │  Run `vibepod run` in any git repo to start.");
    println!("  └\n");

    Ok(())
}

/// `vibepod login` 全体の前提条件（対話端末が必要）の判定。docker や
/// dialoguer を呼ばず TTY 判定だけに依存する分岐なので、`execute` から
/// 切り出してユニットテストできるようにしている（`src/cli/init.rs` の
/// `auto_build_decision` と同じパターン）。
///
/// `login` だけは stdin と stderr の**両方**が TTY であることを AND 条件
/// で要求する。他のコマンド（`init.rs` / `restore.rs`）が stderr のみを
/// 判定しているのとは異なる特殊なケースであり、理由は次のとおり：
///
/// - stdin: `auth::run_setup_token`（`src/auth.rs`）が OAuth フローを
///   `docker exec -it` で実行する際、ホストの `stdin` をコンテナへ接続する
///   （`.stdin(Stdio::inherit())`）。stdin が非 TTY だと `docker exec -it`
///   が `the input device is not a TTY` で失敗する。
/// - stderr: 既存トークンの上書き確認に使う `dialoguer::Confirm` は
///   `Term::stderr()` を使って対話する。stderr が非 TTY だと確認プロンプト
///   を出せない。
///
/// `init.rs` / `restore.rs` は `dialoguer` のみを使い `docker exec -it` を
/// 使わないため stderr のみの判定で正しい。`login` を書き換える際にこの
/// AND 条件を片方だけに戻さないこと（stdin のみの見逃し・stderr のみの
/// 過剰拒否のどちらも再発する）。
fn ensure_interactive_terminal(stdin_is_terminal: bool, stderr_is_terminal: bool) -> Result<()> {
    if stdin_is_terminal && stderr_is_terminal {
        Ok(())
    } else {
        bail!(
            "vibepod login requires an interactive terminal.\n  \
             The OAuth token setup runs `claude setup-token` inside the container via \
             `docker exec -it`, which requires stdin to be a real terminal. Confirming \
             an existing token's overwrite uses a stderr-based prompt, which requires \
             stderr to be a real terminal too. Both are required regardless of whether \
             an existing token is present, so this run is being aborted before starting \
             the container.\n  \
             Re-run `vibepod login` from an interactive terminal."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Issue #68 (W-A): stdin と stderr の両方が TTY のときだけ Ok。
    // OAuth フローが stdin を使う `docker exec -it`（src/auth.rs）と、
    // 上書き確認が stderr を使う `dialoguer::Confirm` の両方に依存する
    // ため、4 象限すべてを固定する。
    #[test]
    fn ensure_interactive_terminal_both_tty_ok() {
        assert!(ensure_interactive_terminal(true, true).is_ok());
    }

    #[test]
    fn ensure_interactive_terminal_stdin_only_errors() {
        // stdin=true, stderr=false: `vibepod login 2> login.log` に相当。
        // 上書き確認の dialoguer::Confirm が stderr を使えないため Err。
        assert!(ensure_interactive_terminal(true, false).is_err());
    }

    #[test]
    fn ensure_interactive_terminal_stderr_only_errors() {
        // stdin=false, stderr=true: `vibepod login < /dev/null` に相当。
        // docker exec -it が stdin を使えず `the input device is not a TTY`
        // で落ちる経路のため、ここで先に Err にする。
        assert!(ensure_interactive_terminal(false, true).is_err());
    }

    #[test]
    fn ensure_interactive_terminal_neither_tty_errors() {
        assert!(ensure_interactive_terminal(false, false).is_err());
    }
}
