use anyhow::{Context, Result};

use crate::config;
use crate::runtime::ContainerConfig;
use crate::session::SessionStore;

mod interactive;
mod prepare;
mod prompt;

/// CLI `run` サブコマンドのオプション
///
/// `vibepod run` コマンドの全引数を保持する。
pub struct RunOptions {
    pub resume: bool,
    pub prompt: Option<String>,
    pub no_network: bool,
    pub env_vars: Vec<String>,
    pub env_file: Option<String>,
    pub lang: Option<String>,
    pub worktree: bool,
    pub review: Option<String>,
    pub mount: Vec<String>,
}

pub(super) struct RunContext {
    pub(super) container_name: String,
    pub(super) effective_workspace: String,
    pub(super) claude_args: Vec<String>,
    pub(super) resolved_env_vars: Vec<String>,
    pub(super) setup_cmd: Option<String>,
    pub(super) temp_claude_json: Option<std::path::PathBuf>,
    pub(super) global_config: config::GlobalConfig,
    pub(super) home: std::path::PathBuf,
    pub(super) worktree_branch_name: Option<String>,
    pub(super) worktree_dir_name: Option<String>,
    pub(super) lang_display: String,
    pub(super) reviewers: Vec<String>,
    pub(super) codex_auth: Option<String>,
    pub(super) store: SessionStore,
    pub(super) deferred_session: crate::session::Session,
    pub(super) extra_mounts: Vec<(String, String)>,
}

pub fn parse_mount_arg(arg: &str) -> anyhow::Result<(String, String)> {
    if let Some((host, container)) = arg.split_once(':') {
        Ok((host.to_string(), container.to_string()))
    } else {
        let path = std::path::Path::new(arg);
        let filename = path
            .file_name()
            .context("Invalid mount path")?
            .to_string_lossy();
        Ok((arg.to_string(), format!("/mnt/{}", filename)))
    }
}

pub fn detect_languages(workspace: &std::path::Path) -> Vec<(String, &'static str)> {
    let mut langs = Vec::new();
    if workspace.join("Cargo.toml").exists() {
        langs.push(("rust".to_string(), "Cargo.toml"));
    }
    if workspace.join("package.json").exists() {
        langs.push(("node".to_string(), "package.json"));
    }
    if workspace.join("go.mod").exists() {
        langs.push(("go".to_string(), "go.mod"));
    }
    if workspace.join("pyproject.toml").exists() {
        langs.push(("python".to_string(), "pyproject.toml"));
    } else if workspace.join("requirements.txt").exists() {
        langs.push(("python".to_string(), "requirements.txt"));
    }
    if workspace.join("pom.xml").exists() {
        langs.push(("java".to_string(), "pom.xml"));
    } else if workspace.join("build.gradle").exists() {
        langs.push(("java".to_string(), "build.gradle"));
    } else if workspace.join("build.gradle.kts").exists() {
        langs.push(("java".to_string(), "build.gradle.kts"));
    }
    langs
}

pub fn get_lang_install_cmd(lang: &str) -> Option<&'static str> {
    match lang {
        "rust" => Some("sudo apt-get update && sudo apt-get install -y build-essential && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && . $HOME/.cargo/env"),
        "node" => Some("curl -fsSL https://deb.nodesource.com/setup_22.x | sudo bash - && sudo apt-get install -y nodejs"),
        "python" => Some("sudo apt-get update && sudo apt-get install -y python3 python3-pip python3-venv"),
        "go" => Some("ARCH=$(uname -m) && GOARCH=$([ \"$ARCH\" = \"aarch64\" ] && echo arm64 || echo amd64) && curl -fsSL https://go.dev/dl/go1.24.2.linux-${GOARCH}.tar.gz | sudo tar -C /usr/local -xzf - && export PATH=$PATH:/usr/local/go/bin"),
        "java" => Some("sudo apt-get update && sudo apt-get install -y default-jdk"),
        _ => None,
    }
}

pub fn validate_slack_channel_id(id: &str) -> bool {
    (id.starts_with('C') || id.starts_with('G')) && id.len() >= 9
}

pub(super) const VALID_REVIEWERS: &[&str] = &["copilot", "codex"];

pub fn resolve_reviewers(review_arg: &Option<String>, config: &[String]) -> Vec<String> {
    match review_arg {
        None => vec![],
        Some(explicit) if !explicit.is_empty() => {
            if VALID_REVIEWERS.contains(&explicit.as_str()) {
                vec![explicit.clone()]
            } else {
                vec![]
            }
        }
        Some(_) => config
            .iter()
            .filter(|r| VALID_REVIEWERS.contains(&r.as_str()))
            .cloned()
            .collect(),
    }
}

pub fn build_review_prompt(prompt: &str, reviewers: &[String]) -> String {
    if reviewers.is_empty() {
        return prompt.to_string();
    }

    let has_codex = reviewers.contains(&"codex".to_string());
    let has_copilot = reviewers.contains(&"copilot".to_string());

    if !has_codex && !has_copilot {
        return prompt.to_string();
    }

    let mut sections: Vec<String> = Vec::new();

    sections.push(
        "## 共通準備\n\
- 現在のブランチが main の場合は `git checkout -b <適切なブランチ名>` で新しいブランチを作成する"
            .to_string(),
    );

    // Codex review フェーズ（ローカル、コミット前）
    if has_codex {
        sections.push(
            "## Codex Review（ローカル、コミット前）\n\
以下を指摘がなくなるまで繰り返す（最大 5 回）:\n\
1. Bash ツールで `codex review -c sandbox_mode=danger-full-access -c approval_policy=never` を実行する（timeout: 600000 を必ず指定すること。デフォルトの 120 秒ではタイムアウトする）\n\
   （重要: Claude Code の内蔵レビュー機能やスキルではなく、Codex CLI コマンドを Bash で直接実行すること。コンテナ内では Linux namespace が使えないため sandbox を無効化し、非対話実行のため approval も無効化する）\n\
2. 出力を確認する。「指摘なし」「no issues」等であればこのフェーズ完了\n\
3. 指摘があれば該当箇所を修正する\n\
4. 手順 1 に戻る"
                .to_string(),
        );
    }

    // コミット + push + PR 作成
    sections.push(
        "## コミットと PR 作成\n\
1. 変更内容をコミットする（Conventional Commits 準拠）\n\
2. `git push -u origin <ブランチ名>` でリモートに push する\n\
3. `gh pr create --base main` で PR を作成する"
            .to_string(),
    );

    // Copilot review フェーズ（PR 上、1ラウンドのみ。API での re-review は未サポート）
    if has_copilot {
        sections.push(
            "## Copilot Review（PR 上、1ラウンド）\n\
1. `gh pr edit <PR番号> --add-reviewer copilot` で Copilot レビューを依頼する\n\
2. 30 秒間隔で最大 10 回 `gh api repos/{owner}/{repo}/pulls/{number}/reviews` をポーリングする\n\
   （重要: `gh pr review` や `gh pr comment` 等の書き込み系コマンドは絶対に使わないこと）\n\
3. レビュー結果を確認する。インラインコメントは `gh api repos/{owner}/{repo}/pulls/{number}/comments` で取得する\n\
4. 指摘があれば修正し、コミットして `git push` する\n\
注意: Copilot の re-review は API から自動でリクエストできないため、1ラウンドで終了する"
                .to_string(),
        );
    }

    sections.push("## 完了\n- 最終的な PR の URL を出力する".to_string());

    format!(
        "{}\n\n---\n\n【必須】上記の作業が終わったら、以下のレビューフローを必ず最後まで実行すること。レビューフローを省略してはならない。\n\n{}",
        prompt,
        sections.join("\n\n")
    )
}

pub(super) fn build_container_config(
    ctx: &RunContext,
    image: String,
    no_network: bool,
) -> ContainerConfig {
    let gitconfig = ctx.home.join(".gitconfig");
    ContainerConfig {
        image,
        container_name: ctx.container_name.clone(),
        workspace_path: ctx.effective_workspace.clone(),
        claude_json: ctx
            .temp_claude_json
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        args: {
            let mut full = vec!["claude".to_string()];
            full.extend(ctx.claude_args.clone());
            full
        },
        gitconfig: if gitconfig.exists() {
            Some(gitconfig.to_string_lossy().to_string())
        } else {
            None
        },
        env_vars: ctx.resolved_env_vars.clone(),
        network_disabled: no_network,
        setup_cmd: ctx.setup_cmd.clone(),
        codex_auth: ctx.codex_auth.clone(),
        extra_mounts: ctx.extra_mounts.clone(),
    }
}

pub async fn execute(opts: RunOptions) -> Result<()> {
    let interactive = !opts.resume && opts.prompt.is_none();

    let Some(ctx) = prepare::prepare_context(&opts).await? else {
        return Ok(());
    };

    if interactive {
        interactive::run_interactive(&opts, &ctx).await
    } else {
        prompt::run_fire_and_forget(&opts, &ctx).await
    }
}
