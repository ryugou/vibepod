use vibepod::report::generate_report;
use vibepod::session::Session;

#[test]
fn test_generate_report() {
    let session = Session {
        id: "20260323-120000-a3f2".to_string(),
        started_at: "2026-03-23T12:00:00+09:00".to_string(),
        head_before: "abc1234".to_string(),
        branch: "main".to_string(),
        prompt: "interactive".to_string(),
        claude_session_path: Some("~/.claude/projects/test/session.jsonl".to_string()),
        restored: false,
    };

    let report = generate_report(
        &session,
        "def5678",
        "def5678 feat: add login page\nccc4444 fix: update styles",
        "A\tsrc/pages/login.rs\nM\tsrc/main.rs",
        " src/pages/login.rs | 45 +++\n src/main.rs | 3 +-\n 2 files changed, 46 insertions(+), 2 deletions(-)",
    );

    assert!(report.contains("# VibePod Session Report"));
    assert!(report.contains("abc1234"));
    assert!(report.contains("def5678"));
    assert!(report.contains("main"));
    assert!(report.contains("interactive"));
    assert!(report.contains("feat: add login page"));
    assert!(report.contains("session.jsonl"));
}

#[test]
fn test_generate_report_no_session_log() {
    let session = Session {
        id: "test".to_string(),
        started_at: "2026-03-23T12:00:00+09:00".to_string(),
        head_before: "abc1234".to_string(),
        branch: "main".to_string(),
        prompt: "interactive".to_string(),
        claude_session_path: None,
        restored: false,
    };

    let report = generate_report(
        &session,
        "def5678",
        "def5678 commit",
        "M\tfile.rs",
        "1 file changed",
    );
    assert!(report.contains("なし"));
}
