use anyhow::{bail, Result};
use dialoguer::{Confirm, Select};

use crate::git;
use crate::report;
use crate::session::SessionStore;

pub fn execute() -> Result<()> {
    let cwd = std::env::current_dir()?;

    // 1. git リポジトリチェック
    if !git::is_git_repo(&cwd) {
        bail!("git リポジトリ内で実行してください");
    }

    // 2. .vibepod/ が git にトラッキングされていないことを確認
    let tracking_check = std::process::Command::new("git")
        .args(["ls-files", ".vibepod"])
        .current_dir(&cwd)
        .output()?;
    if !tracking_check.stdout.is_empty() {
        bail!(".vibepod/ が git 管理下にあります。.gitignore に追加してください");
    }

    // 3. セッション読み込み
    let vibepod_dir = cwd.join(".vibepod");
    let store = SessionStore::new(vibepod_dir);

    let restorable = store.restorable_sessions()?;
    if restorable.is_empty() {
        if store.load()?.sessions.is_empty() {
            bail!("セッション履歴がありません。`vibepod run` を実行してください");
        } else {
            bail!("復元可能なセッションがありません");
        }
    }

    // 4. 未コミット変更チェック
    if git::has_uncommitted_changes(&cwd) {
        bail!("未コミットの変更があります。先にコミットするか stash してください");
    }

    // 5. セッション選択
    println!("\n  ┌  VibePod Restore");
    println!("  │");

    let items: Vec<String> = restorable
        .iter()
        .rev()
        .map(|s| {
            format!(
                "{} ({}) {} - {}",
                s.started_at.get(..19).unwrap_or(&s.started_at),
                s.branch,
                &s.head_before[..7],
                s.prompt
            )
        })
        .collect();

    let selection = Select::new()
        .with_prompt("  ◆  どのセッションに戻しますか？")
        .items(&items)
        .default(0)
        .interact()?;

    // 逆順で表示しているので、インデックスを逆算
    let selected = &restorable[restorable.len() - 1 - selection];

    // 6. HEAD チェック
    let current_head = git::get_head_hash(&cwd)?;

    if current_head == selected.head_before {
        bail!("変更がありません。復元の必要はありません");
    }

    if !git::commit_exists(&cwd, &selected.head_before) {
        bail!(
            "コミット {} が見つかりません。手動で git 操作された可能性があります",
            selected
                .head_before
                .get(..7)
                .unwrap_or(&selected.head_before)
        );
    }

    // 7. ブランチチェック
    let current_branch = git::get_current_branch(&cwd).unwrap_or_default();
    if current_branch != selected.branch {
        println!(
            "  ⚠  セッション時のブランチ（{}）と現在のブランチ（{}）が異なります。",
            selected.branch, current_branch
        );
        if !Confirm::new()
            .with_prompt("  続行しますか？")
            .default(false)
            .interact()?
        {
            println!("  中止しました。");
            return Ok(());
        }
    }

    // 8. 祖先チェック
    if !git::is_ancestor(&cwd, &selected.head_before, &current_head) {
        println!("  ⚠  セッション開始時点のコミットが現在のブランチ履歴上にありません。");
        if !Confirm::new()
            .with_prompt("  強制的に戻しますか？")
            .default(false)
            .interact()?
        {
            println!("  中止しました。");
            return Ok(());
        }
    }

    // 9. 削除対象ファイル表示 + 確認
    println!("  │");
    println!("  ⚠  このセッション以降の全ての変更が巻き戻されます。");

    let untracked = git::get_untracked_files(&cwd)?;
    if !untracked.is_empty() {
        println!("  │");
        println!("  │  以下の未追跡ファイルも削除されます:");
        for f in &untracked {
            println!("  │    {}", f);
        }
    }

    println!("  │");
    if !Confirm::new()
        .with_prompt("  ◆  続行しますか？")
        .default(false)
        .interact()?
    {
        println!("  中止しました。");
        return Ok(());
    }

    // 10. レポート生成
    let commit_log = git::get_commit_log(&cwd, &selected.head_before, &current_head)?;
    let changed_files = git::get_changed_files(&cwd, &selected.head_before, &current_head)?;
    let diff_stat = git::get_diff_stat(&cwd, &selected.head_before, &current_head)?;

    let report_content = report::generate_report(
        selected,
        &current_head,
        &commit_log,
        &changed_files,
        &diff_stat,
    );

    let report_filename = format!("{}.md", chrono::Local::now().format("%Y-%m-%d-%H%M%S"));
    let report_path = store.reports_dir().join(&report_filename);
    std::fs::create_dir_all(store.reports_dir())?;
    std::fs::write(&report_path, &report_content)?;

    println!("  │");
    println!("  ◇  レポートを保存しました: {}", report_path.display());

    // 11. リセット
    println!("  │");
    println!(
        "  ◇  git reset --hard {}",
        selected
            .head_before
            .get(..7)
            .unwrap_or(&selected.head_before)
    );
    git::reset_hard(&cwd, &selected.head_before)?;

    println!("  │  git clean -fd");
    git::clean_fd(&cwd)?;

    // 12. 選択セッション以降の全セッションに restored マーク
    store.mark_restored_since(&selected.id)?;

    println!("  │");
    println!("  ◇  復元完了！");
    println!("  └\n");

    Ok(())
}
