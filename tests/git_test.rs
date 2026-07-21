use std::process::Command;
use tempfile::TempDir;

fn init_test_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "--allow-empty", "-m", "initial"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    dir
}

#[test]
fn test_get_head_hash() {
    let dir = init_test_repo();
    let hash = vibepod::git::get_head_hash(dir.path()).unwrap();
    assert_eq!(hash.len(), 40);
    assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_get_current_branch() {
    let dir = init_test_repo();
    let branch = vibepod::git::get_current_branch(dir.path()).unwrap();
    assert!(!branch.is_empty());
}

#[test]
fn test_is_git_repo() {
    let dir = init_test_repo();
    assert!(vibepod::git::is_git_repo(dir.path()));

    let non_git = TempDir::new().unwrap();
    assert!(!vibepod::git::is_git_repo(non_git.path()));
}

#[test]
fn test_get_remote_url_none() {
    let dir = init_test_repo();
    let remote = vibepod::git::get_remote_url(dir.path());
    assert!(remote.is_none());
}

#[test]
fn test_commit_exists() {
    let dir = init_test_repo();
    let hash = vibepod::git::get_head_hash(dir.path()).unwrap();
    assert!(vibepod::git::commit_exists(dir.path(), &hash));
    assert!(!vibepod::git::commit_exists(
        dir.path(),
        "0000000000000000000000000000000000000000"
    ));
}

#[test]
fn test_is_ancestor() {
    let dir = init_test_repo();
    let first = vibepod::git::get_head_hash(dir.path()).unwrap();
    Command::new("git")
        .args(["commit", "--allow-empty", "-m", "second"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let second = vibepod::git::get_head_hash(dir.path()).unwrap();
    assert!(vibepod::git::is_ancestor(dir.path(), &first, &second));
    assert!(!vibepod::git::is_ancestor(dir.path(), &second, &first));
}

#[test]
fn test_has_uncommitted_changes_clean() {
    let dir = init_test_repo();
    assert!(!vibepod::git::has_uncommitted_changes(dir.path()));
}

#[test]
fn test_has_uncommitted_changes_dirty() {
    let dir = init_test_repo();
    std::fs::write(dir.path().join("file.txt"), "hello").unwrap();
    assert!(vibepod::git::has_uncommitted_changes(dir.path()));
}

#[test]
fn changed_files_reports_unavailable_outside_a_git_repo() {
    // A non-git directory makes `git diff` fail; the result must be the
    // explicit Unavailable, never an empty "no changes" list that would read
    // as "nothing changed" in the run summary.
    let non_git = TempDir::new().unwrap();
    let result = vibepod::git::get_changed_files_since(non_git.path(), "HEAD");
    assert_eq!(result, vibepod::git::ChangedFiles::Unavailable);
}

#[test]
fn changed_files_lists_untracked_and_diff_in_a_repo() {
    let dir = init_test_repo();
    std::fs::write(dir.path().join("first.txt"), "a").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "base"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let base = vibepod::git::get_head_hash(dir.path()).unwrap();

    // One tracked edit and one brand-new untracked file since `base`.
    std::fs::write(dir.path().join("first.txt"), "a-changed").unwrap();
    std::fs::write(dir.path().join("second.txt"), "new").unwrap();

    match vibepod::git::get_changed_files_since(dir.path(), &base) {
        vibepod::git::ChangedFiles::Computed(files) => {
            assert!(files.contains(&"first.txt".to_string()), "got: {:?}", files);
            assert!(
                files.contains(&"second.txt".to_string()),
                "got: {:?}",
                files
            );
        }
        vibepod::git::ChangedFiles::Unavailable => {
            panic!("expected Computed inside a valid repo")
        }
    }
}
