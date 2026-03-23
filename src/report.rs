use crate::session::Session;

pub fn generate_report(
    session: &Session,
    current_head: &str,
    commit_log: &str,
    changed_files: &str,
    diff_stat: &str,
) -> String {
    let session_log = session.claude_session_path.as_deref().unwrap_or("N/A");

    format!(
        r#"# VibePod Session Report

- **Started at:** {}
- **Branch:** {}
- **Mode:** {}
- **HEAD before:** {}
- **HEAD after:** {}
- **Claude session log:** {}

## Commits

{}

## Changed files

{}

## Diff stats

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
