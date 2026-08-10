# profile の可視化とエージェントへの環境告知 実装計画

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 実行中の profile / image を、起動出力・`metadata.json`・コンテナ内エージェントへ渡すプロンプトの3経路へ伝える。

**Architecture:** profile 機構自体は変更しない。`effective_image` の算出を `Session` 構築より前へ前倒しし、起動出力と `Session` の両方が参照できるようにする。エージェントへの告知は純関数 `environment_preamble` が生成し、`build_claude_args` が `claude -p` の値の先頭へ結合する。ロックキー・`Session.prompt`・ログ表示は前置を含めない。

**Tech Stack:** Rust 2021 / clap 4 (derive) / serde / anyhow。テストは統合テスト（`tests/*.rs`）。

**Spec:** `docs/superpowers/specs/2026-08-10-profile-visibility-and-environment-disclosure-design.md`

**Issue:** #63

## Global Constraints

- コミット前に必ず `cargo fmt` を実行する。CI で `cargo fmt --check` が走る。
- `cargo clippy` の警告を解消する。
- 本体コードでの `unwrap()` / `expect()` を禁止する。`?` / `match` / `if let` を使う。テストコード内では許可する。
- `cd` および `git -C` を使わない。作業ディレクトリはリポジトリルート固定とする。
- ブランチは `feat/profile-visibility`（作成済み）。main へ直接コミットしない。
- 前置テキストへツールのバージョン番号を書かない（正本は `templates/Dockerfile` の ARG）。

## File Structure

| ファイル | 責務 | 変更種別 |
| ---- | ---- | ---- |
| `src/cli/run/prepare.rs` | `environment_preamble` の定義、`build_claude_args` のシグネチャ、`effective_image` の算出位置、起動出力、`Session` 構築 | 変更 |
| `src/session.rs` | `Session` への `image` / `profile` フィールド追加 | 変更 |
| `src/cli/mod.rs` | `--lang` ヘルプ文への profile ポインタ | 変更 |
| `tests/environment_preamble_test.rs` | `environment_preamble` と preamble 結合の検証 | 新規 |
| `tests/cli_model_flag.rs` | `build_claude_args` の呼び出し5箇所を新シグネチャへ更新 | 変更 |
| `tests/session_test.rs` | `Session` 構築6箇所へのフィールド追加、後方互換テスト | 変更 |
| `README.md` | Swift profile 節への起動出力例とヘルプ文 | 変更 |
| `docs/design.md` | CLI 出力イメージへの `Profile:` 行、run フローへの前置 | 変更 |

---

### Task 1: `environment_preamble` 純関数

**Files:**
- Modify: `src/cli/run/prepare.rs`（`build_claude_args` の直前へ追加）
- Test: `tests/environment_preamble_test.rs`（新規）

**Interfaces:**
- Consumes: なし
- Produces: `pub fn environment_preamble(profile: Option<&str>, has_package_swift: bool) -> Option<String>`

- [ ] **Step 1: 失敗するテストを書く**

`tests/environment_preamble_test.rs` を新規作成する。

```rust
//! `environment_preamble` は profile と workspace の状態から、コンテナ内
//! エージェントへ渡す環境情報ブロックを導出する純関数。
//! 設計: docs/superpowers/specs/2026-08-10-profile-visibility-and-environment-disclosure-design.md 3.3

use vibepod::cli::run::prepare::environment_preamble;

const PROMPT_DELIMITER: &str = "--- ここから利用者のプロンプト ---";

#[test]
fn swift_profile_returns_available_preamble() {
    let p = environment_preamble(Some("swift"), false).expect("swift profile must produce a preamble");
    assert!(p.contains("導入済み"), "got: {p}");
    assert!(p.ends_with(PROMPT_DELIMITER), "preamble must end with the delimiter; got: {p}");
}

#[test]
fn swift_profile_ignores_package_swift_presence() {
    // 生成規則の表で profile = swift は Package.swift の有無を問わない。
    assert_eq!(
        environment_preamble(Some("swift"), true),
        environment_preamble(Some("swift"), false)
    );
}

#[test]
fn no_profile_with_package_swift_returns_absent_preamble() {
    let p = environment_preamble(None, true).expect("Package.swift without profile must produce a preamble");
    assert!(p.contains("導入されていない"), "got: {p}");
    assert!(p.contains("profile = \"swift\""), "must point to the fix; got: {p}");
    assert!(p.ends_with(PROMPT_DELIMITER), "preamble must end with the delimiter; got: {p}");
}

#[test]
fn no_profile_without_package_swift_returns_none() {
    assert_eq!(environment_preamble(None, false), None);
}

#[test]
fn unknown_profile_returns_none() {
    // VALID_PROFILES へ swift 以外が追加されても、表に無い profile は None。
    assert_eq!(environment_preamble(Some("kotlin"), false), None);
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test --test environment_preamble_test 2>&1 | tail -20`
Expected: コンパイルエラー。`environment_preamble` が見つからない旨。

- [ ] **Step 3: 実装する**

`src/cli/run/prepare.rs` の `build_claude_args` 定義の直前へ追加する。

```rust
/// `profile = "swift"` のコンテナでエージェントへ渡す環境情報。
///
/// バージョン番号を書かない: 正本は `templates/Dockerfile` の ARG であり、
/// ここへ書き写すとイメージ更新のたびに二重管理になる。バージョンが必要な
/// 場合、エージェントはコンテナ内で `swift --version` を実行できる。
const SWIFT_AVAILABLE_PREAMBLE: &str = "[vibepod 環境情報 / 自動付与]
このコンテナには Swift toolchain と SwiftLint が導入済みで、すぐに使える。
- 検証はコンテナ内で実行すること（swift build / swift test / swiftlint lint）。
- toolchain の追加導入は不要。試みてはならない。
- Linux 環境のため、Apple フレームワーク（CryptoKit / SwiftUI / UIKit 等）に依存する
  ターゲットはビルドできない。対象を Foundation のみに依存するパッケージへ限定すること。
- コンテナ内が green でも macOS 側の検証を代替しない。

--- ここから利用者のプロンプト ---";

/// `Package.swift` があるのに profile 未指定のコンテナでエージェントへ渡す
/// 環境情報。自力導入は共有ライブラリ不足で必ず失敗するため、試行そのものを
/// 禁じたうえで恒久対応（config.toml への profile 設定）を示す。
const SWIFT_ABSENT_PREAMBLE: &str = "[vibepod 環境情報 / 自動付与]
このコンテナに Swift toolchain と SwiftLint は導入されていない。
- インストールを試みてはならない。共有ライブラリ不足で失敗し、時間だけを消費する。
- ビルド・テスト・lint は実行せず、最終出力に「未実行」と明記すること。
- 恒久対応: .vibepod/config.toml の [run] へ profile = \"swift\" を設定する。

--- ここから利用者のプロンプト ---";

/// profile と workspace の状態から、エージェントへ渡す環境情報ブロックを
/// 導出する。前置が不要な場合は `None` を返す。
///
/// 生成規則（設計 3.3）:
///
/// | `profile`       | `Package.swift` | 戻り値                     |
/// | --------------- | --------------- | -------------------------- |
/// | `Some("swift")` | 問わない        | `SWIFT_AVAILABLE_PREAMBLE` |
/// | `None`          | あり            | `SWIFT_ABSENT_PREAMBLE`    |
/// | `None`          | なし            | `None`                     |
///
/// `VALID_PROFILES` へ `swift` 以外を追加する場合は、この関数の分岐と対応する
/// 定数を同時に追加すること（追加しない限り新 profile は `None` を返す）。
pub fn environment_preamble(profile: Option<&str>, has_package_swift: bool) -> Option<String> {
    match (profile, has_package_swift) {
        (Some("swift"), _) => Some(SWIFT_AVAILABLE_PREAMBLE.to_string()),
        (None, true) => Some(SWIFT_ABSENT_PREAMBLE.to_string()),
        _ => None,
    }
}
```

- [ ] **Step 4: テストが通ることを確認する**

Run: `cargo test --test environment_preamble_test 2>&1 | tail -20`
Expected: 5 tests passed

- [ ] **Step 5: コミット**

```bash
cargo fmt
git add src/cli/run/prepare.rs tests/environment_preamble_test.rs
git commit -m "feat: add environment_preamble for container tool disclosure"
```

---

### Task 2: `build_claude_args` への preamble 引数追加

**Files:**
- Modify: `src/cli/run/prepare.rs:27-49`（`build_claude_args`）、`src/cli/run/prepare.rs:488`（呼び出し元）
- Modify: `tests/cli_model_flag.rs`（5箇所の呼び出し）
- Test: `tests/environment_preamble_test.rs`（追記）

**Interfaces:**
- Consumes: Task 1 の `environment_preamble`（本タスクでは配線しない。引数を受ける口だけ作る）
- Produces: `pub fn build_claude_args(opts: &RunOptions, interactive: bool, preamble: Option<&str>) -> Vec<String>`

- [ ] **Step 1: 失敗するテストを書く**

`tests/environment_preamble_test.rs` へ追記する。use 宣言はファイル先頭の既存 use と1箇所へまとめ、ファイル途中へ置かない。

```rust
// ファイル先頭: Task 1 の use をこの形へ置き換える
use vibepod::cli::run::prepare::{build_claude_args, environment_preamble};
use vibepod::cli::run::RunOptions;
```

以下をファイル末尾へ追記する。

```rust
fn base_opts() -> RunOptions {
    RunOptions {
        resume: false,
        prompt: Some("do the thing".to_string()),
        prompt_file: None,
        no_network: false,
        env_vars: vec![],
        env_file: None,
        lang: None,
        worktree: false,
        mount: vec![],
        new_container: false,
        update_policy: vibepod::update::UpdatePolicy::default(),
        model: None,
        no_auto_build: false,
        timeout: None,
        verbose: false,
    }
}

/// `-p` の直後の要素（claude へ渡るプロンプト本体）を返す。
fn prompt_value(args: &[String]) -> Option<&str> {
    let idx = args.iter().position(|a| a == "-p")?;
    args.get(idx + 1).map(|s| s.as_str())
}

#[test]
fn preamble_is_prepended_to_prompt() {
    let opts = base_opts();
    let args = build_claude_args(&opts, false, Some("ENV INFO"));
    assert_eq!(prompt_value(&args), Some("ENV INFO\ndo the thing"));
}

#[test]
fn prompt_is_unchanged_without_preamble() {
    let opts = base_opts();
    let args = build_claude_args(&opts, false, None);
    assert_eq!(prompt_value(&args), Some("do the thing"));
}

#[test]
fn preamble_does_not_appear_without_prompt() {
    // 対話モード / --resume は -p を使わないため前置経路を持たない。
    let mut opts = base_opts();
    opts.prompt = None;
    let args = build_claude_args(&opts, true, Some("ENV INFO"));
    assert!(!args.iter().any(|a| a == "-p"), "got: {args:?}");
    assert!(!args.iter().any(|a| a.contains("ENV INFO")), "got: {args:?}");
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test --test environment_preamble_test 2>&1 | tail -20`
Expected: コンパイルエラー。`build_claude_args` の引数が2個であり3個ではない旨。

- [ ] **Step 3: シグネチャと本体を変更する**

`src/cli/run/prepare.rs` の `build_claude_args` を次へ置き換える。doc コメントの既存4項目は残し、`preamble` の項目を追加する。

```rust
/// - `preamble` が `Some` かつ `opts.prompt` が `Some` のとき、`-p` の値を
///   `<preamble>\n<prompt>` とする。前置はこの引数列にのみ現れ、ロックキー・
///   `Session.prompt`・ログ表示は元のプロンプトのままとする（設計 3.5）。
pub fn build_claude_args(
    opts: &RunOptions,
    interactive: bool,
    preamble: Option<&str>,
) -> Vec<String> {
    let mut claude_args: Vec<String> = Vec::new();
    if !interactive {
        claude_args.push("--dangerously-skip-permissions".to_string());
    }
    if let Some(ref model) = opts.model {
        claude_args.push("--model".to_string());
        claude_args.push(model.clone());
    }
    if opts.resume {
        claude_args.push("--resume".to_string());
    }
    if let Some(ref p) = opts.prompt {
        claude_args.push("-p".to_string());
        claude_args.push(match preamble {
            Some(pre) => format!("{pre}\n{p}"),
            None => p.clone(),
        });
        claude_args.push("--output-format".to_string());
        claude_args.push("stream-json".to_string());
        claude_args.push("--verbose".to_string());
    }
    claude_args
}
```

- [ ] **Step 4: 呼び出し元を暫定更新する**

`src/cli/run/prepare.rs:488` を次へ変更する（実際の配線は Task 5）。

```rust
    let claude_args = build_claude_args(opts, interactive, None);
```

- [ ] **Step 5: 既存テストを更新する**

`tests/cli_model_flag.rs` の5箇所を新シグネチャへ更新する。期待値は変更しない。

- 38行: `let args = build_claude_args(&opts, false, None);`
- 49行: `let args = build_claude_args(&opts, true, None);`
- 61行: `let args = build_claude_args(&opts, false, None);`
- 76行: `let args = build_claude_args(&opts, true, None);`
- 90行: `let args = build_claude_args(&opts, false, None);`

- [ ] **Step 6: テストが通ることを確認する**

Run: `cargo test 2>&1 | tail -25`
Expected: 全テスト PASS（`environment_preamble_test` 8件、`cli_model_flag` 5件を含む）

- [ ] **Step 7: コミット**

```bash
cargo fmt
git add src/cli/run/prepare.rs tests/cli_model_flag.rs tests/environment_preamble_test.rs
git commit -m "feat: thread preamble through build_claude_args"
```

---

### Task 3: `effective_image` 算出の前倒しと起動出力への表示

**Files:**
- Modify: `src/cli/run/prepare.rs:188-189`（算出の挿入先）、`src/cli/run/prepare.rs:271-289`（表示）、`src/cli/run/prepare.rs:394-402`（移動元の削除）

**Interfaces:**
- Consumes: なし
- Produces: `effective_image` / `effective_profile` が `prepare_context` 内で `Session` 構築位置（246行）より前に確定する

- [ ] **Step 1: 算出を前倒しする**

`src/cli/run/prepare.rs` の `config::validate_profile(&effective_profile)?;`（189行）の直後へ次を挿入する。

```rust
    // 起動出力（設計 2）と Session 記録（設計 4）が effective_image を参照する
    // ため、global config の読み込みとイメージ名の算出をここへ前倒しする。
    // イメージの自動ビルド（ensure_image_available）は現在位置に残す —
    // ビルド所要時間だけ Session.started_at が後ろへずれるのを避けるため。
    let global_config = config::load_global_config(&config_dir)?;
    // profile 未指定時は現行どおり global_config.image をそのまま使う。
    let effective_image = match effective_profile.as_deref() {
        Some(profile) => config::image_for_profile(&global_config.image, profile),
        None => global_config.image.clone(),
    };
```

- [ ] **Step 2: 移動元を削除する**

`src/cli/run/prepare.rs:394-402` の次のブロックを削除する（`// 3. Check Docker & image` 以降は残す）。削除により手順番号コメントが `1` → `3` と飛ぶため、残る `// 3. Check Docker & image` 以降の番号コメントを1つずつ繰り上げる（`3` → `2`、`4` → `3`、以降同様）。

```rust
    // 2. Load global config (config_dir already loaded at the top).
    let global_config = config::load_global_config(&config_dir)?;

    // profile 未指定時は現行どおり global_config.image をそのまま使う
    // （設計書 2.5 手順2）。
    let effective_image = match effective_profile.as_deref() {
        Some(profile) => config::image_for_profile(&global_config.image, profile),
        None => global_config.image.clone(),
    };
```

- [ ] **Step 3: ビルドが通ることを確認する**

Run: `cargo build 2>&1 | tail -20`
Expected: エラーなし（`global_config` の後続参照が前倒し後の束縛を解決する）

- [ ] **Step 4: 起動出力へ Profile 行を追加する**

`src/cli/run/prepare.rs:271` の `banner::print_banner();` の直前へ profile 表示名を用意し、両分岐へ1行ずつ追加する。

```rust
    // profile 未指定を "default" と表記する。行の有無で判別させないため、
    // profile の指定有無にかかわらず常に出力する（設計 2.2）。
    let profile_label = effective_profile.as_deref().unwrap_or("default");
    banner::print_banner();
    if opts.prompt.is_some() {
        println!();
        println!("Detected git repository: {}", project_name);
        if let Some(ref r) = remote {
            println!("Remote: {}", r);
        }
        println!("Branch: {}", branch);
        println!("Profile: {} (image: {})", profile_label, effective_image);
        println!();
    } else {
        println!("  ┌");
        println!("  │");
        println!("  ◇  Detected git repository: {}", project_name);
        if let Some(ref r) = remote {
            println!("  │  Remote: {}", r);
        }
        println!("  │  Branch: {}", branch);
        println!("  │  Profile: {} (image: {})", profile_label, effective_image);
        println!("  │");
    }
```

- [ ] **Step 5: テストとビルドを確認する**

Run: `cargo test 2>&1 | tail -15`
Expected: 全テスト PASS

Run: `cargo clippy 2>&1 | grep -E "^(warning|error)" | head -10`
Expected: 出力なし

- [ ] **Step 6: コミット**

```bash
cargo fmt
git add src/cli/run/prepare.rs
git commit -m "feat: show Profile and image in run startup output"
```

---

### Task 4: `Session` への image / profile 記録

**Files:**
- Modify: `src/session.rs:8-16`（`Session` struct）
- Modify: `src/cli/run/prepare.rs:246-254`（`deferred_session` 構築）
- Modify: `tests/session_test.rs`（6箇所の `Session` 構築 + 後方互換テスト追加）

**Interfaces:**
- Consumes: Task 3 の `effective_image` / `effective_profile`
- Produces: `Session.image: Option<String>` / `Session.profile: Option<String>`

- [ ] **Step 1: 失敗するテストを書く**

`tests/session_test.rs` の末尾へ追記する。

```rust
/// 既存の metadata.json は image / profile を持たない。フィールド追加後も
/// 読めることを固定する（`vibepod restore` / `logs` / `ps` が既存セッションを
/// 読めなくなる回帰を防ぐ）。
#[test]
fn legacy_metadata_without_image_and_profile_deserializes() {
    let json = r#"{
        "id": "20260806-000943-8f89",
        "started_at": "2026-08-06T00:09:43+09:00",
        "head_before": "bd36ce35c0a460c68c645c6fc841134badd251c7",
        "branch": "main",
        "prompt": "do the thing",
        "claude_session_path": null,
        "restored": false
    }"#;

    let session: Session =
        serde_json::from_str(json).expect("legacy metadata.json must still deserialize");

    assert_eq!(session.image, None);
    assert_eq!(session.profile, None);
    assert_eq!(session.id, "20260806-000943-8f89");
}
```

- [ ] **Step 2: テストが失敗することを確認する**

Run: `cargo test --test session_test 2>&1 | tail -20`
Expected: コンパイルエラー。`Session` に `image` / `profile` が無い旨。

- [ ] **Step 3: `Session` へフィールドを追加する**

`src/session.rs` の `Session` へ次の2フィールドを追加する（`restored` の後）。

```rust
    /// 実行に使用したイメージ名（`RunContext.effective_image`）。
    /// `#[serde(default)]` により、両フィールドを持たない既存の
    /// metadata.json を `None` として読める。
    #[serde(default)]
    pub image: Option<String>,
    /// `[run] profile` の値。未指定は `None`。
    #[serde(default)]
    pub profile: Option<String>,
```

- [ ] **Step 4: 構築箇所へ値を渡す**

`src/cli/run/prepare.rs:246` の `deferred_session` へ2フィールドを追加する。

```rust
    let deferred_session = session::Session {
        id: session_id.clone(),
        started_at: chrono::Local::now().to_rfc3339(),
        head_before,
        branch: current_branch.clone(),
        prompt: prompt_label,
        claude_session_path: None,
        restored: false,
        image: Some(effective_image.clone()),
        profile: effective_profile.clone(),
    };
```

- [ ] **Step 5: 既存テストの構築箇所を更新する**

`tests/session_test.rs` の6箇所（9, 32, 56, 78, 102 行および同ファイル内の残り1箇所）の `Session { ... }` へ次を追加する。

```rust
        image: None,
        profile: None,
```

- [ ] **Step 6: テストが通ることを確認する**

Run: `cargo test 2>&1 | tail -20`
Expected: 全テスト PASS

- [ ] **Step 7: コミット**

```bash
cargo fmt
git add src/session.rs src/cli/run/prepare.rs tests/session_test.rs
git commit -m "feat: record image and profile in session metadata"
```

---

### Task 5: 環境告知の配線

**Files:**
- Modify: `src/cli/run/prepare.rs:341-350`（`Package.swift` 検知）、`src/cli/run/prepare.rs:488`（`build_claude_args` 呼び出し）

**Interfaces:**
- Consumes: Task 1 の `environment_preamble`、Task 2 の `build_claude_args`
- Produces: なし（配線のみ）

- [ ] **Step 1: `Package.swift` の判定を変数化する**

`src/cli/run/prepare.rs:341` の既存ブロックを次へ置き換える。既存コメント（F11 の判定対象に関する説明）は残す。判定を二重に実装せず、警告と前置で同じ値を使う。

```rust
    let has_package_swift = std::path::Path::new(&effective_workspace)
        .join("Package.swift")
        .is_file();
    if effective_profile.is_none() && has_package_swift {
        eprintln!(
            "Note: Detected Package.swift but no `profile` is set. Add `profile = \"swift\"` \
             under [run] in .vibepod/config.toml to use the Swift toolchain image."
        );
    }
```

- [ ] **Step 2: preamble を組み立てて渡す**

`src/cli/run/prepare.rs:488` を次へ置き換える。

```rust
    // コンテナ内エージェントへ環境を伝える経路は claude -p の引数のみ
    // （設計 3.5）。ロックキー・Session.prompt・ログ表示は元のプロンプトを使う。
    let preamble = environment_preamble(effective_profile.as_deref(), has_package_swift);
    let claude_args = build_claude_args(opts, interactive, preamble.as_deref());
```

- [ ] **Step 3: ビルドとテストを確認する**

Run: `cargo test 2>&1 | tail -15`
Expected: 全テスト PASS

Run: `cargo clippy 2>&1 | grep -E "^(warning|error)" | head -10`
Expected: 出力なし

- [ ] **Step 4: コミット**

```bash
cargo fmt
git add src/cli/run/prepare.rs
git commit -m "feat: pass environment preamble to the in-container agent"
```

---

### Task 6: `--help` ポインタとドキュメント横断更新

**Files:**
- Modify: `src/cli/mod.rs:62`（`--lang` の doc コメント）
- Modify: `README.md`（`#### Swift profile` 節）
- Modify: `docs/design.md`（CLI 出力イメージ、run フロー）

**Interfaces:**
- Consumes: Task 3 の起動出力書式
- Produces: なし

- [ ] **Step 1: `--lang` のヘルプ文へポインタを追加する**

`src/cli/mod.rs:62` の doc コメントを次へ置き換える。

```rust
        /// Language toolchain to install in container (rust, node, python, go, java).
        /// Swift toolchain: set `profile = "swift"` in .vibepod/config.toml (see README)
```

- [ ] **Step 2: ヘルプ出力を確認する**

Run: `cargo run -- run --help 2>&1 | grep -A3 'lang'`
Expected: 追加した1行が `--lang` の説明に含まれる

- [ ] **Step 3: README を更新する**

`README.md` の `#### Swift profile` 節へ、起動出力で profile を確認できる旨と実例を追記する。

````markdown
**Verifying the profile is active.** `vibepod run` prints the resolved profile and image at startup, so you can confirm the setting took effect without inspecting containers:

```
Branch: main
Profile: swift (image: vibepod-claude-swift:latest)
```

With no `profile` set, the same line reads `Profile: default (image: vibepod-claude:latest)`.
````

- [ ] **Step 4: `docs/design.md` を更新する**

「CLI 出力イメージ」の該当箇所へ `Profile:` 行を追加し、run フローの記述へ次を追記する。

```markdown
- `--prompt` 実行時、`claude -p` へ渡すプロンプトの先頭へ環境情報を自動前置する（profile 由来。ロックキー・`Session.prompt`・ログ表示には含めない）。正本: `docs/superpowers/specs/2026-08-10-profile-visibility-and-environment-disclosure-design.md`
```

- [ ] **Step 5: 全テストとフォーマットを確認する**

Run: `cargo fmt --check 2>&1 | tail -5`
Expected: 出力なし

Run: `cargo test 2>&1 | tail -15`
Expected: 全テスト PASS

- [ ] **Step 6: コミット**

```bash
git add src/cli/mod.rs README.md docs/design.md
git commit -m "docs: point --help at the swift profile and document startup output"
```

---

## E2E 検証（実装完了後、ホストで実施）

ユニットテストの対象外。実装者はここまで完了後に報告し、E2E は受理判断者が実施する。

1. `profile = "swift"` のプロジェクト（`/Users/ryugo/Developer/src/personal/kaokakushi`）で `vibepod run --prompt` を実行し、起動出力が `Profile: swift (image: vibepod-claude-swift:latest)` を含み、`.vibepod/sessions/<id>/metadata.json` に `image` / `profile` が記録されること。
2. profile 未指定かつ `Package.swift` を持つ検証用リポジトリで、前置 (b) がエージェントへ渡ること。検証用リポジトリは次で用意する。

```bash
mkdir -p ~/.claude/jobs/tmp/vibepod-preamble-e2e && touch ~/.claude/jobs/tmp/vibepod-preamble-e2e/Package.swift && git init -q ~/.claude/jobs/tmp/vibepod-preamble-e2e
```

`vibepod run --prompt "この環境で swift build を実行できるか、実行せずに答えよ"` を実行し、エージェントの応答が「toolchain 無し・インストール禁止」を認識していること、および `Note: Detected Package.swift` の警告が stderr へ出ることを確認する。
3. profile 未指定かつ `Package.swift` の無いリポジトリ（vibepod 自身）で、起動出力が `Profile: default (image: vibepod-claude:latest)` となり、前置が付かないこと。
