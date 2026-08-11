use anyhow::{bail, Context, Result};

use crate::auth::{self, AuthManager, TokenData};
use crate::config;
use crate::runtime::DockerRuntime;

pub async fn execute() -> Result<()> {
    println!("\n  ┌  VibePod Login");
    println!("  │");

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
            // dialoguer の `Confirm` は `Term::stderr()` を使って対話するため
            // （`src/cli/init.rs` の `execute` と同じ理由）、上書き確認を出す前に
            // stderr の TTY を確認する。`vibepod login` は OAuth フローで対話が
            // 本質的に必要なコマンドであり、自動で「はい」と答えてトークンを
            // 上書きするのは許容できないため、非 TTY ではフォールバックせず
            // エラーで中断する。既存トークンが無い経路にはこの確認プロンプトが
            // 無いため、この関数は呼ばれず非 TTY でも問題なく完走する。
            ensure_overwrite_confirmable(std::io::IsTerminal::is_terminal(&std::io::stderr()))?;
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

/// 既存トークンの上書き確認を出せるかどうかの判定。docker や dialoguer を
/// 呼ばず TTY 判定だけに依存する分岐なので、`execute` から切り出して
/// ユニットテストできるようにしている（`src/cli/init.rs` の
/// `auto_build_decision` と同じパターン）。stderr が TTY でなければ
/// `dialoguer::Confirm` を呼ばずにここで中断する。
fn ensure_overwrite_confirmable(stderr_is_terminal: bool) -> Result<()> {
    if stderr_is_terminal {
        Ok(())
    } else {
        bail!(
            "vibepod login requires an interactive terminal to confirm overwriting the \
             existing token.\n  \
             An existing token was found, and overwriting it requires a confirmation that \
             cannot be obtained because stderr is not a terminal, so this run is being \
             aborted without touching the existing token.\n  \
             Re-run `vibepod login` from an interactive terminal, or run `vibepod logout` \
             first to remove the existing token and then re-run `vibepod login`."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Issue #68: 非 TTY かつ既存トークンあり（＝この関数が呼ばれる経路）は
    // エラーで中断する。
    #[test]
    fn ensure_overwrite_confirmable_non_terminal_errors() {
        assert!(ensure_overwrite_confirmable(false).is_err());
    }

    #[test]
    fn ensure_overwrite_confirmable_terminal_ok() {
        assert!(ensure_overwrite_confirmable(true).is_ok());
    }

    // 非 TTY かつ既存トークンなしのケース: `execute` はこの関数を一切呼ばず、
    // 確認プロンプトの無い経路をそのまま通るため正常継続する。この経路は
    // `dialoguer` を実際に呼ぶ分岐ではなくトークン読み込み結果（`AuthManager`
    // 経由でファイルI/Oが絡む）に依存するため、ユニットテストの対象外とする。
}
