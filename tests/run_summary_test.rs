//! `--timeout` パースと `--prompt` 実行後の要約レンダリング（純関数）の検証。

use vibepod::cli::run::{
    parse_timeout_secs, render_run_summary, render_timeout_message, DEFAULT_OVERALL_TIMEOUT_SECS,
};
use vibepod::git::ChangedFiles;

/// Shorthand: a successfully-computed changed-file list.
fn computed(files: &[&str]) -> ChangedFiles {
    ChangedFiles::Computed(files.iter().map(|s| s.to_string()).collect())
}

// --- parse_timeout_secs ---

#[test]
fn timeout_bare_seconds() {
    assert_eq!(parse_timeout_secs("1800").unwrap(), 1800);
}

#[test]
fn timeout_zero_means_disabled() {
    assert_eq!(parse_timeout_secs("0").unwrap(), 0);
}

#[test]
fn timeout_duration_minutes() {
    assert_eq!(parse_timeout_secs("30m").unwrap(), 30 * 60);
}

#[test]
fn timeout_duration_compound() {
    assert_eq!(parse_timeout_secs("1h30m").unwrap(), 90 * 60);
}

#[test]
fn timeout_duration_seconds_suffix() {
    assert_eq!(parse_timeout_secs("90s").unwrap(), 90);
}

#[test]
fn timeout_trims_whitespace() {
    assert_eq!(parse_timeout_secs("  45m ").unwrap(), 45 * 60);
}

#[test]
fn timeout_empty_is_error() {
    assert!(parse_timeout_secs("   ").is_err());
}

#[test]
fn timeout_garbage_is_error() {
    let err = parse_timeout_secs("soon").unwrap_err();
    assert!(
        err.to_string().contains("invalid --timeout"),
        "expected actionable error, got: {}",
        err
    );
}

#[test]
fn timeout_default_is_thirty_minutes() {
    assert_eq!(DEFAULT_OVERALL_TIMEOUT_SECS, 30 * 60);
}

// --- render_run_summary ---

#[test]
fn summary_success_lists_status_and_logs() {
    let changed = computed(&["src/main.rs", "README.md"]);
    let out = render_run_summary(
        true,
        "success",
        Some("All checks pass."),
        &changed,
        "/w/.vibepod/sessions/abc/logs.txt",
    );
    assert!(out.contains("Status: success"), "got: {}", out);
    assert!(out.contains("Result: All checks pass."), "got: {}", out);
    assert!(out.contains("Changed files (2):"), "got: {}", out);
    assert!(out.contains("src/main.rs"), "got: {}", out);
    assert!(out.contains("README.md"), "got: {}", out);
    assert!(
        out.contains("Full logs: /w/.vibepod/sessions/abc/logs.txt"),
        "got: {}",
        out
    );
}

#[test]
fn summary_failure_shows_reason() {
    let out = render_run_summary(
        false,
        "error_max_turns",
        None,
        &computed(&[]),
        "/tmp/logs.txt",
    );
    assert!(
        out.contains("Status: failed (error_max_turns)"),
        "got: {}",
        out
    );
}

#[test]
fn summary_no_changes_says_none() {
    let out = render_run_summary(true, "success", None, &computed(&[]), "/tmp/logs.txt");
    assert!(out.contains("Changed files: (none)"), "got: {}", out);
}

#[test]
fn summary_unavailable_is_distinct_from_none() {
    // 算出不能は「変更なし (none)」と別文言で表示され、誤って無変更に
    // 見えないこと（指摘 #2 の不変条件）。
    let out = render_run_summary(
        true,
        "success",
        None,
        &ChangedFiles::Unavailable,
        "/tmp/logs.txt",
    );
    assert!(
        !out.contains("Changed files: (none)"),
        "unavailable must not read as 'none': {}",
        out
    );
    assert!(
        out.contains("could not be computed"),
        "unavailable must be explicit: {}",
        out
    );
}

#[test]
fn summary_omits_empty_result_text() {
    // 空白のみの result 本文は Result 行を出さない。
    let out = render_run_summary(
        true,
        "success",
        Some("   "),
        &computed(&[]),
        "/tmp/logs.txt",
    );
    assert!(!out.contains("Result:"), "got: {}", out);
}

#[test]
fn summary_always_includes_logs_path() {
    // 生ログの保存先は常に提示される（要約が生ログの代替になるため）。
    let out = render_run_summary(
        false,
        "whatever",
        None,
        &computed(&[]),
        "/some/where/logs.txt",
    );
    assert!(
        out.contains("Full logs: /some/where/logs.txt"),
        "got: {}",
        out
    );
}

// --- render_timeout_message ---
//
// 要件2（設計書 第3節）: タイムアウト時は workspace を一切変更しない。この
// 関数は純関数であり git コマンドを一切呼ばないため、呼び出すだけで
// 「git 操作を行わない」ことを構造的に保証できる。呼び出し元（prompt.rs）が
// read-only な git 呼び出しで求めた `head_advanced` / `has_uncommitted` を
// 引数として渡す設計なので、ここでは状態の組み合わせごとに出力される案内が
// 正しく出し分けられることを検証する。
//
// F2（フル再レビュー Major 指摘）: `vibepod restore` は未コミット変更が
// 残っていると必ず bail するため、無条件に案内すると到達不能なコマンドを
// 勧めてしまっていた。cwd（`worktree_dir = None`）での3分岐:
//   (a) has_uncommitted = true         → restore 不可の理由 + 破棄/保持の選択肢
//   (b) has_uncommitted = false かつ head_advanced = true → 従来通り restore 案内
//   (c) どちらでもない                  → 「変更なし」のみ
//
// F3（フル再レビュー Major 指摘）: `--worktree` 実行では成果物が
// `.worktrees/<dir>` にあり `vibepod restore` は適用できないため、
// `worktree_dir = Some(..)` のときは cwd 前提の案内を一切出さず、
// `git -C .worktrees/<dir>` 経由の案内に置き換える。

fn timeout_msg(head_advanced: bool, has_uncommitted: bool, worktree_dir: Option<&str>) -> String {
    render_timeout_message(
        false,
        900,
        1800,
        Some(std::path::Path::new("/tmp/logs.txt")),
        head_advanced,
        has_uncommitted,
        worktree_dir,
    )
}

#[test]
fn timeout_message_never_claims_an_automatic_reset() {
    // 状態の組み合わせによらず、workspace が自動でリセットされたと誤読
    // させる文言を含んではならない（要件2の不変条件）。
    //
    // MJ1（フル再レビュー指摘）で破棄コマンドを `git reset --hard` に
    // 変更したため、素の "reset" という単語自体は（バッククォート付きの
    // 手動実行コマンドとして）メッセージに正当に現れるようになった。
    // このテストが本来防ぎたいのは「vibepod が自動でリセットした」という
    // 過去形の断定であって、「あなたが `git reset --hard` を実行すれば
    // 戻せる」という手動コマンドの案内ではない。そのため判定を、自動実行を
    // 意味するフレーズの不在に絞る。
    let automatic_claim_phrases = [
        "自動でリセット",
        "自動的にリセット",
        "リセットされました",
        // 旧実装が実際に出力していた能動形（「作業ディレクトリを {} に
        // リセットしました。」）。最も可能性の高い回帰形なので明示的に禁止する。
        "リセットしました",
        "リセット済み",
        "automatically reset",
        "has been reset",
    ];
    for head_advanced in [false, true] {
        for has_uncommitted in [false, true] {
            for worktree_dir in [None, Some("vibepod-prompt-20260803-000000")] {
                let out = timeout_msg(head_advanced, has_uncommitted, worktree_dir);
                for phrase in automatic_claim_phrases {
                    assert!(
                        !out.contains(phrase),
                        "must not claim an automatic reset happened (found {phrase:?}, \
                         head_advanced={head_advanced}, has_uncommitted={has_uncommitted}, \
                         worktree_dir={worktree_dir:?}): {out}"
                    );
                }
            }
        }
    }
}

// --- cwd（worktree_dir = None）: F2 の3分岐 ---

#[test]
fn cwd_with_uncommitted_changes_says_restore_is_unavailable_and_offers_discard_or_keep() {
    let out = timeout_msg(true, true, None);
    assert!(
        out.contains("git status") && out.contains("git log"),
        "must still tell the operator how to inspect the workspace: {}",
        out
    );
    assert!(
        out.contains("`vibepod restore` は未コミットの変更が残っていると実行できません"),
        "must explain why `vibepod restore` cannot be used here: {}",
        out
    );
    assert!(
        out.contains("git reset --hard && git clean -fd") && out.contains("取り消し不能"),
        "must offer the discard command and flag it as irreversible: {}",
        out
    );
    assert!(
        // MJ1（フル再レビュー指摘）回帰: `git checkout .` は index から
        // working tree を復元するだけでステージ済み変更を戻さず、
        // `git clean -fd` も `git add` 済みの新規ファイル（index 入り =
        // tracked 扱い）を消せない。案内どおり破棄しても
        // restore.rs:47 の bail が再現していた。引数無し `git reset --hard`
        // は HEAD を動かさず index + working tree の両方を HEAD の状態へ
        // 戻すため、この問題が起きない。
        !out.contains("git checkout ."),
        "must not offer the flawed `git checkout .` discard command (leaves staged changes \
         behind): {}",
        out
    );
    assert!(
        out.contains("git add -A && git commit"),
        "must offer a way to keep the changes (commit then restore): {}",
        out
    );
}

#[test]
fn cwd_committed_and_clean_recommends_vibepod_restore() {
    let out = timeout_msg(true, false, None);
    assert!(
        out.contains("git status") && out.contains("git log"),
        "must tell the operator how to inspect the workspace: {}",
        out
    );
    assert!(
        out.contains("開始時点へ戻すには: `vibepod restore`"),
        "must point to the manual recovery command when it is actually usable: {}",
        out
    );
}

#[test]
fn cwd_no_changes_says_nothing_changed_and_does_not_mention_restore() {
    let out = timeout_msg(false, false, None);
    assert!(
        out.contains("開始時点から workspace に変更はありません"),
        "must say plainly that nothing changed: {}",
        out
    );
    assert!(
        !out.contains("vibepod restore"),
        "must not suggest restoring when there is nothing to restore: {}",
        out
    );
}

// --- worktree（worktree_dir = Some）: F3 ---

const WORKTREE_DIR: &str = "vibepod-prompt-20260803-120000";

#[test]
fn worktree_with_uncommitted_changes_uses_git_dash_c_discard_not_vibepod_restore() {
    let out = timeout_msg(true, true, Some(WORKTREE_DIR));
    assert!(
        out.contains(&format!(".worktrees/{}", WORKTREE_DIR)),
        "must name the worktree path: {}",
        out
    );
    assert!(
        out.contains(&format!("git -C .worktrees/{} status", WORKTREE_DIR)),
        "must offer a git -C status command scoped to the worktree: {}",
        out
    );
    assert!(
        // Q3（simplify 指摘）回帰: cwd/worktree の3状態テンプレートが二重
        // 実装だったせいで、worktree の「未コミットあり」分岐にだけ `log`
        // が無かった（cwd 側は status/log の両方を案内していた）。確認
        // コマンドは両モードで対称であるべき。
        out.contains(&format!("git -C .worktrees/{} log", WORKTREE_DIR)),
        "must offer a git -C log command scoped to the worktree (symmetry with cwd, which \
         offers both status and log here): {}",
        out
    );
    assert!(
        out.contains(&format!("git -C .worktrees/{} reset --hard", WORKTREE_DIR))
            && out.contains("取り消し不能"),
        "must offer a worktree-scoped discard command flagged as irreversible: {}",
        out
    );
    assert!(
        // MJ1 回帰（worktree 版）: cwd 側と同じ理由で `checkout .` は
        // ステージ済み変更を戻せない。
        !out.contains("checkout ."),
        "must not offer the flawed `checkout .` discard command: {}",
        out
    );
    assert!(
        !out.contains("vibepod restore"),
        "vibepod restore operates on cwd, not the worktree, and must not be suggested: {}",
        out
    );
    assert!(
        // mn1（フル再レビュー指摘）: 未コミットの変更が残っている worktree に
        // 対して素の `git worktree remove` は
        // "contains modified or untracked files, use --force to delete it"
        // で必ず失敗する。この分岐では --force 付きの案内でなければならない。
        out.contains(&format!(
            "git worktree remove --force .worktrees/{}",
            WORKTREE_DIR
        )),
        "must offer --force when the worktree has uncommitted changes (plain `git worktree \
         remove` fails on a dirty worktree): {}",
        out
    );
}

#[test]
fn worktree_committed_and_clean_uses_git_dash_c_diff_not_vibepod_restore() {
    let out = timeout_msg(true, false, Some(WORKTREE_DIR));
    assert!(
        out.contains(&format!("git -C .worktrees/{} log", WORKTREE_DIR)),
        "must offer a way to inspect the worktree's commits: {}",
        out
    );
    assert!(
        // Q3（simplify 指摘）回帰: worktree の「コミット済みのみ・clean」
        // 分岐にだけ `status` が無かった（cwd 側は status/log の両方を
        // 案内していた）。確認コマンドは両モードで対称であるべき。
        out.contains(&format!("git -C .worktrees/{} status", WORKTREE_DIR)),
        "must offer a git -C status command scoped to the worktree (symmetry with cwd, which \
         offers both status and log here): {}",
        out
    );
    assert!(
        out.contains(&format!("git -C .worktrees/{} diff main", WORKTREE_DIR)),
        "must offer a way to diff the worktree branch against main: {}",
        out
    );
    assert!(
        !out.contains("vibepod restore"),
        "vibepod restore operates on cwd, not the worktree, and must not be suggested: {}",
        out
    );
    assert!(
        // mn1: ツリーが clean（コミット済みのみ）なら `git worktree remove`
        // は素のまま成功するため、--force を勧める必要はない。
        out.contains(&format!("git worktree remove .worktrees/{}", WORKTREE_DIR))
            && !out.contains("--force"),
        "must offer plain `git worktree remove` (no --force needed on a clean worktree): {}",
        out
    );
}

#[test]
fn worktree_no_changes_still_names_the_worktree_path() {
    let out = timeout_msg(false, false, Some(WORKTREE_DIR));
    assert!(
        out.contains(&format!(".worktrees/{}", WORKTREE_DIR)),
        "must name the worktree path even when nothing changed: {}",
        out
    );
    assert!(
        out.contains("開始時点から変更はありません"),
        "must say plainly that nothing changed: {}",
        out
    );
    assert!(
        !out.contains("vibepod restore"),
        "vibepod restore operates on cwd, not the worktree, and must not be suggested: {}",
        out
    );
    assert!(
        // mn1: 変更が無いツリーも clean なので --force は不要。
        out.contains(&format!("git worktree remove .worktrees/{}", WORKTREE_DIR))
            && !out.contains("--force"),
        "must offer plain `git worktree remove` (no --force needed when nothing changed): {}",
        out
    );
}

// --- タイムアウト表示（ラベル・上限値） ---

#[test]
fn timeout_message_idle_uses_idle_limit() {
    let out = render_timeout_message(false, 300, 1800, None, true, false, None);
    assert!(
        out.contains("ストリーム無出力"),
        "idle timeout should be labeled as such: {}",
        out
    );
    assert!(
        out.contains("5 分"),
        "300s idle limit should read as 5 分: {}",
        out
    );
}

#[test]
fn timeout_message_overall_uses_overall_limit() {
    let out = render_timeout_message(true, 300, 1800, None, true, false, None);
    assert!(
        out.contains("実時間"),
        "overall timeout should be labeled as such: {}",
        out
    );
    assert!(
        out.contains("30 分"),
        "1800s overall limit should read as 30 分: {}",
        out
    );
}

#[test]
fn timeout_message_includes_log_path_when_present() {
    let out = render_timeout_message(
        true,
        300,
        60,
        Some(std::path::Path::new("/w/logs.txt")),
        true,
        false,
        None,
    );
    assert!(out.contains("/w/logs.txt"), "got: {}", out);
}

#[test]
fn timeout_message_omits_log_path_when_absent() {
    let out = render_timeout_message(true, 300, 60, None, true, false, None);
    assert!(
        !out.contains("ログ:"),
        "no log line should be printed when no path is available: {}",
        out
    );
}

#[test]
fn timeout_message_under_a_minute_uses_seconds() {
    let out = render_timeout_message(true, 300, 45, None, true, false, None);
    assert!(out.contains("45 秒"), "got: {}", out);
}

#[test]
fn timeout_message_with_remainder_seconds_shows_minutes_and_seconds() {
    // F9 回帰: `limit_secs / 60` の整数除算だけだと 90 秒が「1 分」と表示され、
    // 切り捨てられた 30 秒の端数が運用者に伝わらない（実際には 1 分 30 秒
    // 待たされたのに「1 分」と言われると、タイムアウト設定値の見直しを
    // 誤らせる）。端数がある場合は分と秒の両方を表示すること。
    let out = render_timeout_message(true, 300, 90, None, true, false, None);
    assert!(
        out.contains("1 分 30 秒"),
        "90s should read as '1 分 30 秒', not silently truncate the remainder: {}",
        out
    );
}
