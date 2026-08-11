use anyhow::{bail, Context, Result};

use crate::auth::{self, AuthManager, TokenData};
use crate::config;
use crate::runtime::DockerRuntime;

pub async fn execute() -> Result<()> {
    println!("\n  ┌  VibePod Login");
    println!("  │");

    // `vibepod login` は対話コマンドとして、常に対話端末（stdin と stderr の
    // 両方が TTY であること）を要求する（製品判断）。
    //
    // 技術的な必要性は非対称: stdin は `auth::run_setup_token`
    // （`src/auth.rs`）が `docker exec -it` で OAuth フローを実行する際に
    // ホストの端末へ接続するため常に必要。stderr は既存の有効なトークンが
    // ある場合の上書き確認 `dialoguer::Confirm`（`Term::stderr()` を使う）
    // でのみ技術的に必要になり、トークンが無い・期限切れの場合はこの
    // プロンプト自体が実行されない。
    //
    // それでもここでは既存トークンの有無で判定を分岐せず、常に両方を
    // AND 条件で要求する。前提条件を単純に保つための意図的な選択であり、
    // 詳細と理由は `ensure_interactive_terminal` の doc コメントを参照。
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
/// 判定しているのとは異なる特殊なケースである。
///
/// 技術的な必要性は次のとおり非対称:
///
/// - stdin: 常に必要。`auth::run_setup_token`（`src/auth.rs`）が OAuth
///   フローを `docker exec -it` で実行する際、ホストの `stdin` をコンテナ
///   へ接続する（`.stdin(Stdio::inherit())`）。stdin が非 TTY だと
///   `docker exec -it` が `the input device is not a TTY` で失敗する。
/// - stderr: 有効な既存トークンがあり、上書き確認 `dialoguer::Confirm`
///   （`Term::stderr()` を使う）を実行するときにだけ技術的に必要になる。
///   トークンが無い、または期限切れの場合は Confirm 自体が実行されない
///   ため、stderr が非 TTY でも OAuth フローは技術的には完走できる。
///
/// にもかかわらずこの関数は既存トークンの有無で判定を分岐せず、常に
/// 両方を要求する。これは技術的制約ではなく製品判断である。
/// `vibepod login` は対話コマンドという前提を単純に保ち、
/// 「上書き確認が必要か」を判定入力に混ぜて分岐とテストを倍増させない
/// 選択をしている。`vibepod login 2>login.log`（ブラウザで OAuth 認証
/// しながら stderr だけリダイレクトする）のような運用は実運用でほぼ
/// 発生せず、判定を分岐する複雑さに見合わないため。
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
             `vibepod login` is an interactive command by design. The OAuth token setup \
             runs `claude setup-token` inside the container via `docker exec -it`, which \
             always requires stdin to be a real terminal. Confirming an existing token's \
             overwrite uses a stderr-based prompt, which is only technically needed when \
             a valid token already exists -- but this command intentionally requires both \
             stdin and stderr to be real terminals regardless, to keep its precondition \
             simple. This run is being aborted before starting the container.\n  \
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
