use anyhow::{bail, Result};
use dialoguer::{Confirm, Select};

use crate::git;
use crate::report;
use crate::session::SessionStore;

pub fn execute() -> Result<()> {
    let cwd = std::env::current_dir()?;

    // 1. Git repo check
    if !git::is_git_repo(&cwd) {
        bail!("Not a git repository. Run this command inside a git-initialized directory.");
    }

    // 2. Ensure .vibepod/ is ignored by git (not just untracked)
    let tracking_check = std::process::Command::new("git")
        .args(["ls-files", ".vibepod"])
        .current_dir(&cwd)
        .output()?;
    if !tracking_check.stdout.is_empty() {
        bail!(".vibepod/ is tracked by git. Add it to .gitignore.");
    }
    let ignore_check = std::process::Command::new("git")
        .args(["check-ignore", "-q", ".vibepod"])
        .current_dir(&cwd)
        .output()?;
    if !ignore_check.status.success() {
        bail!(".vibepod/ is not in .gitignore. Add it to prevent `git clean` from deleting session history.");
    }

    // 3. Load sessions
    let vibepod_dir = cwd.join(".vibepod");
    let store = SessionStore::new(vibepod_dir);

    let restorable = store.restorable_sessions()?;
    if restorable.is_empty() {
        if store.load()?.sessions.is_empty() {
            bail!("No session history found. Run `vibepod run` first.");
        } else {
            bail!("No restorable sessions available.");
        }
    }

    // 4. Check for uncommitted changes
    if git::has_uncommitted_changes(&cwd) {
        bail!("Uncommitted changes detected. Please commit or stash them first.");
    }

    // 5. Select session
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
        .with_prompt("  ◆  Select session to restore")
        .items(&items)
        .default(0)
        .interact()?;

    let selected = &restorable[restorable.len() - 1 - selection];

    // 6. HEAD check
    let current_head = git::get_head_hash(&cwd)?;

    if current_head == selected.head_before {
        bail!("No changes to restore.");
    }

    if !git::commit_exists(&cwd, &selected.head_before) {
        bail!(
            "Commit {} not found. The repository may have been modified manually.",
            selected
                .head_before
                .get(..7)
                .unwrap_or(&selected.head_before)
        );
    }

    // 7. Branch check
    let current_branch = git::get_current_branch(&cwd).unwrap_or_default();
    if current_branch != selected.branch {
        println!(
            "  ⚠  Current branch ({}) differs from session branch ({}).",
            current_branch, selected.branch
        );
        if !Confirm::new()
            .with_prompt("  Continue?")
            .default(false)
            .interact()?
        {
            println!("  Aborted.");
            return Ok(());
        }
    }

    // 8. Ancestor check
    if !git::is_ancestor(&cwd, &selected.head_before, &current_head) {
        println!("  ⚠  Session start commit is not in the current branch history.");
        if !Confirm::new()
            .with_prompt("  Force restore?")
            .default(false)
            .interact()?
        {
            println!("  Aborted.");
            return Ok(());
        }
    }

    // 9. Show files to be removed + confirm
    println!("  │");
    println!("  ⚠  All changes after this session will be reverted.");

    let untracked = git::get_untracked_files(&cwd)?;
    if !untracked.is_empty() {
        println!("  │");
        println!("  │  The following untracked files will be deleted:");
        for f in &untracked {
            println!("  │    {}", f);
        }
    }

    println!("  │");
    if !Confirm::new()
        .with_prompt("  ◆  Continue?")
        .default(false)
        .interact()?
    {
        println!("  Aborted.");
        return Ok(());
    }

    // 10. Generate report
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

    let session_dir = store.session_dir(&selected.id);
    std::fs::create_dir_all(&session_dir)?;
    let report_path = session_dir.join("report.md");
    std::fs::write(&report_path, report_content)?;

    println!("  │");
    println!("  ◇  Report saved: {}", report_path.display());

    // 11. Reset
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

    // 12. Mark selected and subsequent sessions as restored
    store.mark_restored_since(&selected.id)?;

    println!("  │");
    println!("  ◇  Restore complete!");
    println!("  └\n");

    Ok(())
}
