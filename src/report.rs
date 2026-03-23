use crate::session::Session;

pub fn generate_report(
    session: &Session,
    current_head: &str,
    commit_log: &str,
    changed_files: &str,
    diff_stat: &str,
) -> String {
    let session_log = session.claude_session_path.as_deref().unwrap_or("なし");

    format!(
        r#"# VibePod Session Report

- **実行日時:** {}
- **ブランチ:** {}
- **モード:** {}
- **開始HEAD:** {}
- **終了HEAD:** {}
- **Claude セッションログ:** {}

## コミット一覧

{}

## 変更ファイル一覧

{}

## 変更統計

{}
"#,
        session.started_at,
        session.branch,
        session.prompt,
        session.head_before,
        current_head,
        session_log,
        commit_log,
        changed_files,
        diff_stat,
    )
}
