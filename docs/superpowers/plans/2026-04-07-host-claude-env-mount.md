# Host Claude Env Mount Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Mount host `~/.claude/plugins/` and a sanitized `~/.claude/settings.json` into the vibepod container so that user-installed plugins (e.g. `codex`) and enabled-plugin settings work inside the container without hanging.

**Architecture:** Extend the existing `build_claude_config_mounts` function in `src/cli/run/mod.rs` to add two new read-only mount entries for plugins (one at `/home/vibepod/.claude/plugins` and one at `<host_home>/.claude/plugins` to resolve absolute `installPath` entries inside `installed_plugins.json`). Add a new `sanitize_settings_json` function that reads `~/.claude/settings.json`, strips host-local fields (`hooks`, `statusLine`), writes the result under `~/.config/vibepod/runtime/<container-name>/settings.json`, and returns a mount entry. Wire the sanitized-settings generation into `prepare_context` alongside the existing `build_claude_config_mounts` call. No Dockerfile changes.

**Tech Stack:** Rust (anyhow, serde_json, std::fs, tempfile for tests), Docker bind mounts

---

## File Structure

| Action | File | Responsibility |
|--------|------|---------------|
| Modify | `src/cli/run/mod.rs` | Add plugins to `build_claude_config_mounts`; add `sanitize_settings_json` + `prepare_sanitized_settings_mount` helpers |
| Modify | `src/cli/run/prepare.rs` | Call `prepare_sanitized_settings_mount` after `build_claude_config_mounts`; extend the label computation to include the sanitized settings mount target |
| Modify | `tests/run_logic_test.rs` | Tests for plugins entries and settings sanitization |
| Modify | `README.md` | Update Security Model mount list |
| Modify | `docs/design.md` | Update mount description |

---

## Task 1: Extend `build_claude_config_mounts` with plugins (two mount entries)

**Files:**
- Modify: `src/cli/run/mod.rs:125-154`
- Modify: `tests/run_logic_test.rs:100-143`

- [ ] **Step 1: Write failing test for plugins mount (two entries)**

Add to `tests/run_logic_test.rs` after the existing `test_claude_config_mounts_partial` test:

```rust
#[test]
fn test_claude_config_mounts_includes_plugins_at_both_paths() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join(".claude");
    std::fs::create_dir_all(claude_dir.join("plugins")).unwrap();

    let mounts = build_claude_config_mounts(dir.path());

    let plugins_host = claude_dir.join("plugins").to_string_lossy().to_string();
    let host_home_str = dir.path().to_string_lossy().to_string();
    let absolute_container_path = format!("{}/.claude/plugins", host_home_str);

    // Mount at /home/vibepod/.claude/plugins (where $HOME/.claude/plugins is read)
    assert!(
        mounts
            .iter()
            .any(|(src, dst)| src == &plugins_host && dst == "/home/vibepod/.claude/plugins"),
        "expected plugins mounted at /home/vibepod/.claude/plugins, got {:?}",
        mounts
    );

    // Mount at host-absolute path (where installed_plugins.json installPath points)
    assert!(
        mounts
            .iter()
            .any(|(src, dst)| src == &plugins_host && dst == &absolute_container_path),
        "expected plugins mounted at {}, got {:?}",
        absolute_container_path,
        mounts
    );
}

#[test]
fn test_claude_config_mounts_skips_plugins_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    // Intentionally no plugins/ directory

    let mounts = build_claude_config_mounts(dir.path());

    assert!(
        !mounts.iter().any(|(_, dst)| dst.ends_with("/plugins")),
        "expected no plugins mounts when ~/.claude/plugins is absent, got {:?}",
        mounts
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Users/ryugo/Developer/src/personal/vibepod && cargo test --test run_logic_test test_claude_config_mounts_includes_plugins 2>&1 | tail -20`

Expected: FAIL — mounts.len() is 1 (only the tempdir), no plugins entries produced by current implementation.

- [ ] **Step 3: Extend `build_claude_config_mounts` to add plugins**

In `src/cli/run/mod.rs`, replace the function body (lines 125-154) with:

```rust
/// `~/.claude/` 配下のグローバル設定ファイル・ディレクトリのマウント定義を構築する。
/// 存在するもののみ含まれる。read-only でマウントされる。
///
/// `plugins/` は特殊で、2 つのマウント先を返す:
/// 1. `/home/vibepod/.claude/plugins` — Claude Code が $HOME 経由で読む先
/// 2. `<host_home>/.claude/plugins` — `installed_plugins.json` 内の `installPath`
///    フィールドがホスト絶対パスを持つため、同じ絶対パスに再マウントして解決する
pub fn build_claude_config_mounts(home: &std::path::Path) -> Vec<(String, String)> {
    let claude_dir = home.join(".claude");
    let mut mounts = Vec::new();

    let claude_md = claude_dir.join("CLAUDE.md");
    if claude_md.is_file() {
        mounts.push((
            claude_md.to_string_lossy().to_string(),
            "/home/vibepod/.claude/CLAUDE.md".to_string(),
        ));
    }

    let skills_dir = claude_dir.join("skills");
    if skills_dir.is_dir() {
        mounts.push((
            skills_dir.to_string_lossy().to_string(),
            "/home/vibepod/.claude/skills".to_string(),
        ));
    }

    let agents_dir = claude_dir.join("agents");
    if agents_dir.is_dir() {
        mounts.push((
            agents_dir.to_string_lossy().to_string(),
            "/home/vibepod/.claude/agents".to_string(),
        ));
    }

    let plugins_dir = claude_dir.join("plugins");
    if plugins_dir.is_dir() {
        let plugins_host = plugins_dir.to_string_lossy().to_string();
        // (1) Claude Code が $HOME/.claude/plugins として読む先
        mounts.push((
            plugins_host.clone(),
            "/home/vibepod/.claude/plugins".to_string(),
        ));
        // (2) installed_plugins.json の installPath フィールドはホスト絶対パスを
        //     保持しているため、同じ絶対パスに再マウントして解決する
        let absolute_container_path = format!("{}/.claude/plugins", home.to_string_lossy());
        mounts.push((plugins_host, absolute_container_path));
    }

    mounts
}
```

- [ ] **Step 4: Run the new tests to verify they pass**

Run: `cd /Users/ryugo/Developer/src/personal/vibepod && cargo test --test run_logic_test test_claude_config_mounts 2>&1 | tail -20`

Expected: All `test_claude_config_mounts_*` tests pass (including the new 2 plus the existing 3).

- [ ] **Step 5: Run full test suite to confirm no regressions**

Run: `cd /Users/ryugo/Developer/src/personal/vibepod && cargo test 2>&1 | tail -10`

Expected: All tests pass.

- [ ] **Step 6: Format and lint**

Run: `cd /Users/ryugo/Developer/src/personal/vibepod && cargo fmt && cargo clippy -- -D warnings 2>&1 | tail -5`

Expected: No warnings.

- [ ] **Step 7: Commit**

```bash
cd /Users/ryugo/Developer/src/personal/vibepod
git add src/cli/run/mod.rs tests/run_logic_test.rs
git commit -m "$(cat <<'EOF'
feat: mount ~/.claude/plugins into vibepod container

Adds two bind-mount entries for ~/.claude/plugins when it exists on the
host: one at /home/vibepod/.claude/plugins (where $HOME/.claude/plugins
is read) and one at <host_home>/.claude/plugins to resolve the host
absolute paths stored in installed_plugins.json.

This allows user-installed plugins (e.g. codex) to be available inside
the vibepod container without requiring them to be baked into the image.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Add `sanitize_settings_json` function

**Files:**
- Modify: `src/cli/run/mod.rs` (add sanitize function)
- Modify: `tests/run_logic_test.rs` (add sanitize tests)

- [ ] **Step 1: Write failing tests for sanitize_settings_json**

Add to `tests/run_logic_test.rs` imports at the top:

```rust
use vibepod::cli::run::{
    build_claude_config_mounts, detect_languages, get_lang_install_cmd, parse_mount_arg,
    sanitize_settings_json, validate_slack_channel_id,
};
```

Then add these tests at the end of the file:

```rust
// --- sanitize_settings_json ---

#[test]
fn test_sanitize_settings_strips_hooks() {
    let input = r#"{
        "env": {"FOO": "bar"},
        "permissions": {"allow": ["Bash(ls:*)"]},
        "hooks": {
            "Notification": [
                {"matcher": "", "hooks": [{"type": "command", "command": "/Users/x/.claude/hooks/n.sh"}]}
            ]
        },
        "enabledPlugins": {"codex@openai-codex": true}
    }"#;

    let sanitized = sanitize_settings_json(input).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&sanitized).unwrap();

    assert!(parsed.get("hooks").is_none(), "hooks should be stripped");
    assert!(parsed.get("env").is_some(), "env should be preserved");
    assert!(parsed.get("permissions").is_some(), "permissions should be preserved");
    assert!(parsed.get("enabledPlugins").is_some(), "enabledPlugins should be preserved");
    assert_eq!(
        parsed["enabledPlugins"]["codex@openai-codex"],
        serde_json::Value::Bool(true)
    );
}

#[test]
fn test_sanitize_settings_strips_status_line() {
    let input = r#"{
        "env": {},
        "statusLine": {"type": "command", "command": "/Users/x/.claude/bin/status.sh"}
    }"#;

    let sanitized = sanitize_settings_json(input).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&sanitized).unwrap();

    assert!(parsed.get("statusLine").is_none(), "statusLine should be stripped");
    assert!(parsed.get("env").is_some(), "env should be preserved");
}

#[test]
fn test_sanitize_settings_preserves_unknown_fields() {
    let input = r#"{
        "env": {"X": "1"},
        "teammateMode": "tmux",
        "extraKnownMarketplaces": {"foo": {"source": {"source": "github", "repo": "a/b"}}}
    }"#;

    let sanitized = sanitize_settings_json(input).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&sanitized).unwrap();

    assert_eq!(parsed["teammateMode"], serde_json::Value::String("tmux".to_string()));
    assert!(parsed.get("extraKnownMarketplaces").is_some());
}

#[test]
fn test_sanitize_settings_empty_object() {
    let sanitized = sanitize_settings_json("{}").unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&sanitized).unwrap();
    assert!(parsed.is_object());
    assert_eq!(parsed.as_object().unwrap().len(), 0);
}

#[test]
fn test_sanitize_settings_invalid_json_errors() {
    let result = sanitize_settings_json("not valid json {");
    assert!(result.is_err(), "invalid JSON should return an error");
}
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cd /Users/ryugo/Developer/src/personal/vibepod && cargo test --test run_logic_test test_sanitize_settings 2>&1 | tail -20`

Expected: FAIL — `sanitize_settings_json` is not defined yet (compile error).

- [ ] **Step 3: Add `sanitize_settings_json` function**

In `src/cli/run/mod.rs`, add this function right after `build_claude_config_mounts`:

```rust
/// ホストの `~/.claude/settings.json` を読み、コンテナに持ち込めない
/// ホスト固有フィールドを除去した JSON 文字列を返す。
///
/// 除去対象:
/// - `hooks` — 絶対パスでホストスクリプトを参照するため
/// - `statusLine` — 同様にホストスクリプトを参照する可能性があるため
///
/// その他のフィールド（`env`, `permissions`, `enabledPlugins`,
/// `extraKnownMarketplaces`, `teammateMode` 等）はそのまま保持する。
pub fn sanitize_settings_json(input: &str) -> anyhow::Result<String> {
    let mut value: serde_json::Value = serde_json::from_str(input)
        .context("Failed to parse settings.json")?;

    if let Some(obj) = value.as_object_mut() {
        obj.remove("hooks");
        obj.remove("statusLine");
    }

    serde_json::to_string_pretty(&value)
        .context("Failed to serialize sanitized settings.json")
}
```

Make sure `use anyhow::Context;` is present at the top of the file. If not, add it. Check existing imports first:

Run: `head -10 src/cli/run/mod.rs`
If `Context` is not imported, change the existing `use anyhow::` line to include it, e.g. `use anyhow::{Context, Result};`.

- [ ] **Step 4: Run the sanitize tests to verify they pass**

Run: `cd /Users/ryugo/Developer/src/personal/vibepod && cargo test --test run_logic_test test_sanitize_settings 2>&1 | tail -20`

Expected: All 5 sanitize tests pass.

- [ ] **Step 5: Run full test suite**

Run: `cd /Users/ryugo/Developer/src/personal/vibepod && cargo test 2>&1 | tail -10`

Expected: All tests pass.

- [ ] **Step 6: Format and lint**

Run: `cd /Users/ryugo/Developer/src/personal/vibepod && cargo fmt && cargo clippy -- -D warnings 2>&1 | tail -5`

Expected: No warnings.

- [ ] **Step 7: Commit**

```bash
cd /Users/ryugo/Developer/src/personal/vibepod
git add src/cli/run/mod.rs tests/run_logic_test.rs
git commit -m "$(cat <<'EOF'
feat: add sanitize_settings_json helper

Strips host-specific fields (hooks, statusLine) from ~/.claude/settings.json
so the sanitized copy can be safely mounted into the container. Other
fields (env, permissions, enabledPlugins, etc.) are preserved as-is.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Wire sanitized settings.json into prepare.rs

**Files:**
- Modify: `src/cli/run/prepare.rs` around lines 520-532 (where `extra_mounts` is assembled)
- Modify: `src/cli/run/mod.rs` (add `prepare_sanitized_settings_mount` helper)
- Modify: `tests/run_logic_test.rs` (test for prepare_sanitized_settings_mount)

- [ ] **Step 1: Write failing test for prepare_sanitized_settings_mount**

Add to `tests/run_logic_test.rs` imports:

```rust
use vibepod::cli::run::prepare_sanitized_settings_mount;
```

Add test:

```rust
// --- prepare_sanitized_settings_mount ---

#[test]
fn test_prepare_sanitized_settings_mount_writes_and_returns_entry() {
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();

    // Create a fake ~/.claude/settings.json with hooks to be stripped
    let claude_dir = home_dir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{"env":{"X":"1"},"hooks":{"Notification":[]}}"#,
    )
    .unwrap();

    let result = prepare_sanitized_settings_mount(
        home_dir.path(),
        config_dir.path(),
        "vibepod-test-abc123",
    )
    .unwrap();

    let (host_path, container_path) = result.expect("should return a mount entry");

    assert_eq!(container_path, "/home/vibepod/.claude/settings.json");
    assert!(
        host_path.contains("vibepod-test-abc123"),
        "host path should include container name: {}",
        host_path
    );

    // Verify the file was written and is sanitized
    let written = std::fs::read_to_string(&host_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert!(parsed.get("hooks").is_none(), "hooks should be stripped in written file");
    assert!(parsed.get("env").is_some(), "env should be preserved");
}

#[test]
fn test_prepare_sanitized_settings_mount_no_host_settings() {
    let home_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    // No .claude/settings.json on host

    let result = prepare_sanitized_settings_mount(
        home_dir.path(),
        config_dir.path(),
        "vibepod-test-none",
    )
    .unwrap();

    assert!(result.is_none(), "should return None when host settings.json is absent");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd /Users/ryugo/Developer/src/personal/vibepod && cargo test --test run_logic_test test_prepare_sanitized_settings_mount 2>&1 | tail -20`

Expected: FAIL — function not defined (compile error).

- [ ] **Step 3: Implement `prepare_sanitized_settings_mount` in `src/cli/run/mod.rs`**

Add after `sanitize_settings_json`:

```rust
/// ホストの `~/.claude/settings.json` をサニタイズしたコピーを生成し、
/// コンテナにマウントするためのマウントエントリを返す。
///
/// サニタイズ済み JSON は `<config_dir>/runtime/<container_name>/settings.json`
/// に書き出される。この場所は vibepod が書き込み許可を持つ唯一の場所である。
///
/// ホスト側の `settings.json` が存在しない場合は `None` を返す（マウント追加不要）。
pub fn prepare_sanitized_settings_mount(
    home: &std::path::Path,
    config_dir: &std::path::Path,
    container_name: &str,
) -> anyhow::Result<Option<(String, String)>> {
    let host_settings = home.join(".claude").join("settings.json");
    if !host_settings.is_file() {
        return Ok(None);
    }

    let raw = std::fs::read_to_string(&host_settings)
        .with_context(|| format!("Failed to read {}", host_settings.display()))?;
    let sanitized = sanitize_settings_json(&raw)?;

    let runtime_dir = config_dir.join("runtime").join(container_name);
    std::fs::create_dir_all(&runtime_dir)
        .with_context(|| format!("Failed to create {}", runtime_dir.display()))?;

    let target = runtime_dir.join("settings.json");
    std::fs::write(&target, sanitized)
        .with_context(|| format!("Failed to write {}", target.display()))?;

    Ok(Some((
        target.to_string_lossy().to_string(),
        "/home/vibepod/.claude/settings.json".to_string(),
    )))
}
```

- [ ] **Step 4: Run the new test to verify it passes**

Run: `cd /Users/ryugo/Developer/src/personal/vibepod && cargo test --test run_logic_test test_prepare_sanitized_settings_mount 2>&1 | tail -20`

Expected: Both `test_prepare_sanitized_settings_mount_*` tests pass.

- [ ] **Step 5: Wire it into `prepare.rs`**

In `src/cli/run/prepare.rs`, find the block around lines 528-532:

```rust
    // ~/.claude/ 配下のグローバル設定をマウント対象に追加（存在する場合のみ）
    let claude_config_mounts = super::build_claude_config_mounts(&home);
    for (host, container) in &claude_config_mounts {
        extra_mounts.push((host.clone(), container.clone()));
    }
```

Replace with:

```rust
    // ~/.claude/ 配下のグローバル設定をマウント対象に追加（存在する場合のみ）
    let claude_config_mounts = super::build_claude_config_mounts(&home);
    for (host, container) in &claude_config_mounts {
        extra_mounts.push((host.clone(), container.clone()));
    }

    // ホスト ~/.claude/settings.json をサニタイズしてマウント対象に追加
    if let Some((host, container)) =
        super::prepare_sanitized_settings_mount(&home, &config_dir, &container_name)?
    {
        extra_mounts.push((host, container));
    }
```

- [ ] **Step 6: Update the label computation to include the sanitized settings mount**

In `src/cli/run/prepare.rs`, find the block around lines 449-460 where `claude_config_mounts_for_label` is built:

```rust
    // 9b. 設定変更の検知（env ファイル解決後に env ハッシュを含めて比較）
    // ~/.claude/ マウントも含めるため、home を先に解決する
    let home_early_for_mounts = crate::config::home_dir()?;
    let claude_config_mounts_for_label = super::build_claude_config_mounts(&home_early_for_mounts);

    if let Some(stored_labels) = stored_labels_opt {
        let mut mounts_parts: Vec<String> = Vec::new();
        for arg in &opts.mount {
            if let Ok((h, c)) = parse_mount_arg(arg) {
                mounts_parts.push(format!("{}:{}", h, c));
            }
        }
        for (h, c) in &claude_config_mounts_for_label {
            mounts_parts.push(format!("{}:{}", h, c));
        }
```

Add the sanitized settings container path to `mounts_parts` for label calculation. Because the host path is a per-container runtime temp file, we only include the container destination path in the label hash so that stale runtime files don't force recreates:

Replace the `if let Some(stored_labels) = stored_labels_opt {` block's `mounts_parts` construction so it looks like:

```rust
    if let Some(stored_labels) = stored_labels_opt {
        let mut mounts_parts: Vec<String> = Vec::new();
        for arg in &opts.mount {
            if let Ok((h, c)) = parse_mount_arg(arg) {
                mounts_parts.push(format!("{}:{}", h, c));
            }
        }
        for (h, c) in &claude_config_mounts_for_label {
            mounts_parts.push(format!("{}:{}", h, c));
        }
        // Sanitized settings: include only container destination in the label so
        // regenerated host-side runtime files do not trigger a spurious recreate
        let host_settings_exists = home_early_for_mounts
            .join(".claude")
            .join("settings.json")
            .is_file();
        if host_settings_exists {
            mounts_parts.push(":/home/vibepod/.claude/settings.json".to_string());
        }
        mounts_parts.sort();
```

- [ ] **Step 7: Run full test suite to check for regressions**

Run: `cd /Users/ryugo/Developer/src/personal/vibepod && cargo test 2>&1 | tail -20`

Expected: All tests pass.

- [ ] **Step 8: Format and lint**

Run: `cd /Users/ryugo/Developer/src/personal/vibepod && cargo fmt && cargo clippy -- -D warnings 2>&1 | tail -5`

Expected: No warnings.

- [ ] **Step 9: Commit**

```bash
cd /Users/ryugo/Developer/src/personal/vibepod
git add src/cli/run/mod.rs src/cli/run/prepare.rs tests/run_logic_test.rs
git commit -m "$(cat <<'EOF'
feat: mount sanitized ~/.claude/settings.json into container

Generates a sanitized copy of the host's ~/.claude/settings.json
(stripping hooks and statusLine which reference host absolute paths)
and mounts it at /home/vibepod/.claude/settings.json so that enabled
plugins and other safe fields are respected inside the container.

The sanitized copy is stored at
~/.config/vibepod/runtime/<container_name>/settings.json.

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Manual E2E verification

**Files:** (none — manual steps)

- [ ] **Step 1: Rebuild image if needed**

Run: `cd /Users/ryugo/Developer/src/personal/vibepod && cargo build --release 2>&1 | tail -5`

Expected: Build succeeds.

Note: Dockerfile is unchanged, so `vibepod init` is not required unless the image is missing.

- [ ] **Step 2: Stop and remove any existing vibepod container for this project**

Run: `/Users/ryugo/Developer/src/personal/vibepod/target/release/vibepod ps`

If an existing container exists, run: `/Users/ryugo/Developer/src/personal/vibepod/target/release/vibepod rm <container-name>`

- [ ] **Step 3: Start a fresh interactive container**

Run this in a separate terminal (interactive TTY required):
```
cd /Users/ryugo/Developer/src/personal/vibepod && ./target/release/vibepod run --new
```

Expected:
- Output lists the new mounts under "Mount (ro):" including plugins (two entries), and the sanitized settings.json
- `claude` starts without hanging
- The prompt accepts input

- [ ] **Step 4: Verify the plugin is available in the container**

Inside the interactive `claude` session started in Step 3, type `/` and confirm that `/codex:review` (or whichever plugin slash command the user normally uses) appears in the list.

If it does not appear, check:
- `docker exec <container-name> ls -la /home/vibepod/.claude/plugins/`
- `docker exec <container-name> ls -la /Users/ryugo/.claude/plugins/`
- `docker exec <container-name> cat /home/vibepod/.claude/settings.json`

**This is a user-facing verification — ask the user to confirm before proceeding to Task 5.**

- [ ] **Step 5: Verify host files are untouched**

Run: `ls -la ~/.claude/plugins/installed_plugins.json && md5sum ~/.claude/plugins/installed_plugins.json`

Compare before/after the vibepod run (should be byte-identical since mounts are read-only).

- [ ] **Step 6: Exit the container and confirm clean shutdown**

Type `exit` in the claude session. The container should stop cleanly.

Run: `/Users/ryugo/Developer/src/personal/vibepod/target/release/vibepod ps`

Expected: The container is listed as stopped (not running).

---

## Task 5: Update docs

**Files:**
- Modify: `README.md` (Security Model → Minimal mounts section)
- Modify: `docs/design.md`

- [ ] **Step 1: Update README.md Security Model mount list**

In `README.md`, find the "Minimal mounts" bullet list under "Security Model" (around lines 150-156):

```markdown
2. **Minimal mounts** — only what the agent needs is mounted:
   - `$(pwd)` → `/workspace` (read-write): your project files
   - `~/.claude.json` → container via **temporary copy** (read-write): onboarding state; the host file is never written directly
   - `~/.gitconfig` → `/home/vibepod/.gitconfig` (read-only): git user name and email
   - `--mount`-specified paths (read-only): additional host paths you explicitly opt in
   - `~/.codex/auth.json` (read-only, when `--review codex` is used): Codex authentication
   - `GH_TOKEN` injected from `gh auth token` when available, for GitHub CLI access inside the container
```

Add new bullets for host Claude env mounts (insert after the `~/.gitconfig` line):

```markdown
   - `~/.claude/CLAUDE.md`, `~/.claude/skills/`, `~/.claude/agents/` (read-only, when present): your personal Claude Code instructions, skills, and agents
   - `~/.claude/plugins/` (read-only, when present): your installed Claude Code plugins — mounted at both `/home/vibepod/.claude/plugins` and the host absolute path to resolve `installed_plugins.json` entries
   - `~/.claude/settings.json` via **sanitized copy** (read-only, when present): a per-container copy with `hooks` and `statusLine` stripped, written to `~/.config/vibepod/runtime/<container>/settings.json`
```

- [ ] **Step 2: Update docs/design.md mount section**

Find the mount section in `docs/design.md` and add a paragraph describing the host Claude env mount policy:

```markdown
### ホスト Claude 環境の取り込み

ユーザーがホストで使っている Claude Code 環境（プラグイン・skill・agent・グローバル CLAUDE.md・settings）をコンテナ内でも使えるように、`~/.claude/` 配下のサブディレクトリを選択的に read-only でマウントする。`~/.claude/` 全体のマウントは過去にハングが観測されたため禁止する（詳細は `docs/superpowers/specs/2026-03-23-vibepod-auth-design.md` を参照）。

マウント対象:
- `~/.claude/CLAUDE.md` → `/home/vibepod/.claude/CLAUDE.md`
- `~/.claude/skills/` → `/home/vibepod/.claude/skills`
- `~/.claude/agents/` → `/home/vibepod/.claude/agents`
- `~/.claude/plugins/` → `/home/vibepod/.claude/plugins` および `<host_home>/.claude/plugins`（二重マウント）
- `~/.claude/settings.json` → `/home/vibepod/.claude/settings.json`（sanitize 済みコピー）

plugins の二重マウントは、`installed_plugins.json` の `installPath` フィールドがホスト絶対パスを持つことへの対処。settings.json のサニタイズでは、ホストスクリプトを絶対パスで参照する `hooks` と `statusLine` フィールドを除去する。
```

If `docs/design.md` already has a mount section, add this as a subsection. Otherwise add it at the end.

- [ ] **Step 3: Commit**

```bash
cd /Users/ryugo/Developer/src/personal/vibepod
git add README.md docs/design.md
git commit -m "$(cat <<'EOF'
docs: document host ~/.claude mount policy

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review Checklist (run after all tasks complete)

- [ ] Spec coverage: every item in `docs/specs/v1.5/2026-04-07-host-claude-env-mount.md` § 設計方針 has a corresponding task
- [ ] No placeholders remain (`TBD`, `TODO`, `fill in details`, etc.)
- [ ] Type / function-name consistency: `build_claude_config_mounts`, `sanitize_settings_json`, `prepare_sanitized_settings_mount` are referenced identically across plan, code, and tests
- [ ] `cargo fmt && cargo clippy -- -D warnings` passes
- [ ] `cargo test` passes
- [ ] Manual E2E verification (Task 4) confirmed by user before requesting review
- [ ] `/codex:review` flow followed before opening PR (user-invoked)
- [ ] PR created against main with a link to this plan and the spec
