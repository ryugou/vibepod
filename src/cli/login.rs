use anyhow::{bail, Context, Result};

use crate::auth::{self, AuthManager, TokenData};
use crate::config;
use crate::runtime::DockerRuntime;

pub async fn execute() -> Result<()> {
    println!("\n  ┌  VibePod Login");
    println!("  │");

    // トークン取得の実体である `auth::run_setup_token`（`src/auth.rs`）は、
    // OAuth フローを `docker exec -it` で実行し、`stdin` もホストの端末へ
    // 接続する（`src/auth.rs` の該当箇所）。`docker exec -it` は TTY が無い
    // 環境では `the input device is not a TTY` で失敗するため、既存トークン
    // の有無にかかわらずこのコマンドは非 TTY 環境では完走できない。上書き
    // 確認だけをガードしても、トークンが無い場合は docker 由来の分かりにくい
    // エラーで落ちてしまうため、コマンド全体の前提条件としてここで 1 回だけ
    // 判定する。
    ensure_interactive_terminal(std::io::IsTerminal::is_terminal(&std::io::stderr()))?;

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
/// `auto_build_decision` と同じパターン）。stderr が TTY でなければ、
/// 上書き確認の `dialoguer::Confirm` はもちろん、その先にある
/// `auth::run_setup_token`（`docker exec -it` で OAuth フローを実行する）
/// にも到達させず、ここで中断する。
fn ensure_interactive_terminal(stderr_is_terminal: bool) -> Result<()> {
    if stderr_is_terminal {
        Ok(())
    } else {
        bail!(
            "vibepod login requires an interactive terminal.\n  \
             The OAuth token setup runs `claude setup-token` inside the container via \
             `docker exec -it`, which needs a real terminal for both input and output \
             regardless of whether an existing token is present, so this run is being \
             aborted before starting the container.\n  \
             Re-run `vibepod login` from an interactive terminal."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Issue #68: 非 TTY はエラーで中断する（既存トークンの有無に関係なく、
    // OAuth フロー自体が `docker exec -it` を使うため）。
    #[test]
    fn ensure_interactive_terminal_non_terminal_errors() {
        assert!(ensure_interactive_terminal(false).is_err());
    }

    #[test]
    fn ensure_interactive_terminal_terminal_ok() {
        assert!(ensure_interactive_terminal(true).is_ok());
    }
}
