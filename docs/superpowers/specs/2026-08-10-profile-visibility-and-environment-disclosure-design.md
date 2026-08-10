# profile の可視化とエージェントへの環境告知 設計

| 項目 | 内容 |
| ---- | ---- |
| 目的 | 実行中の profile / image を利用者とコンテナ内エージェントの双方へ伝える手段を定める |
| 読者 | 実装エージェント（kaneko）、レビュアー |
| 正本の範囲 | 起動出力の表示項目、環境告知テキストの生成規則と適用範囲、`Session` の追加フィールド、`--help` の記載 |
| 関連文書 | [`2026-08-03-swift-profile-and-session-hardening-design.md`](2026-08-03-swift-profile-and-session-hardening-design.md)（profile 機構の正本）、`templates/Dockerfile`（同梱ツールのバージョンの正本） |

## 1. 課題

profile 機構は関連文書の設計どおり動作しており、本設計は機構自体を変更しない。塞ぐのは、profile が有効かどうかを確認する手段が `docker ps -a` しか無いことによる、次の2経路の情報欠落である。

- 利用者へ: 起動出力に profile / image が出ない。設定が効いているかを実行時に判別できない。
- コンテナ内エージェントへ: 環境を伝える経路が無く、運用プロンプトの手書き記述だけが根拠になる。記述が古びると、利用可能な toolchain を封じたまま実行される。

要件は4つとする。

| 要件 | 内容 | 優先度 |
| ---- | ---- | ---- |
| 1 | 起動出力への profile / image 表示 | 高 |
| 2 | プロンプトへの環境情報自動前置 | 高 |
| 3 | `Session` への image / profile 記録 | 中 |
| 4 | `vibepod run --help` への profile ポインタ | 低 |

## 2. 要件1: 起動出力への profile / image 表示

### 2.1 採用する設計

`prepare_context`（`src/cli/run/prepare.rs`）のリポジトリ検出ブロックへ1行を追加する。profile の指定有無にかかわらず常に出力する。

同ブロックは `opts.prompt.is_some()` で2つの出力形式に分岐する。**両方の分岐へ追加する**。片方だけに追加すると、対話モードと `--prompt` モードのどちらかで確認できなくなる。

`--prompt` モード（平文形式。`Branch:` の直後）:

```
Branch: feat/7-export-flow
Profile: swift (image: vibepod-claude-swift:latest)
```

対話モード（`◇` 形式。`│  Branch:` の直後）:

```
  │  Branch: feat/7-export-flow
  │  Profile: swift (image: vibepod-claude-swift:latest)
```

profile 未指定時は、いずれの形式でも `Profile: default (image: vibepod-claude:latest)` とする。

### 2.2 不変条件

- 表示するイメージ名は `RunContext.effective_image` と一致する。`global_config.image` を直接読まない（profile が無視された値を表示しないため）。
- profile 未指定は `default` と表記する。行の省略や空文字にしない。

「設定が効いていない」状態を、効いている状態と同じ位置・同じ形式で示す。行の有無による判別を利用者に強いない。

## 3. 要件2: プロンプトへの環境情報自動前置

### 3.1 採用する設計

`claude -p` へ渡すプロンプト文字列の先頭へ、profile と workspace の状態から導出した環境情報ブロックを付与する。

### 3.2 インターフェース

```rust
/// profile と workspace の状態から、エージェントへ渡す環境情報ブロックを
/// 導出する。前置が不要な場合は None を返す。
pub fn environment_preamble(profile: Option<&str>, has_package_swift: bool) -> Option<String>;

pub fn build_claude_args(
    opts: &RunOptions,
    interactive: bool,
    preamble: Option<&str>,
) -> Vec<String>;
```

### 3.3 生成規則

| `profile` | `Package.swift` | 戻り値 |
| ---- | ---- | ---- |
| `Some("swift")` | 問わない | 3.4 (a) |
| `None` | あり | 3.4 (b) |
| `None` | なし | `None` |

`has_package_swift` は `effective_workspace` 直下の `Package.swift` の有無とする（`--worktree` 実行時は worktree 内が対象）。`prepare.rs` の既存 Package.swift 検知と同一の判定結果を用い、判定を二重に実装しない。

`VALID_PROFILES` に `swift` 以外が追加された場合、その profile は表に無いため `None` を返す。profile 追加時は本節の表と 3.4 のテキストを同時に更新する。

### 3.4 前置テキスト

(a) `profile = "swift"`:

```
[vibepod 環境情報 / 自動付与]
このコンテナには Swift toolchain と SwiftLint が導入済みで、すぐに使える。
- 検証はコンテナ内で実行すること（swift build / swift test / swiftlint lint）。
- toolchain の追加導入は不要。試みてはならない。
- Linux 環境のため、Apple フレームワーク（CryptoKit / SwiftUI / UIKit 等）に依存する
  ターゲットはビルドできない。対象を Foundation のみに依存するパッケージへ限定すること。
- コンテナ内が green でも macOS 側の検証を代替しない。

--- ここから利用者のプロンプト ---
```

(b) `profile` 未指定かつ `Package.swift` あり:

```
[vibepod 環境情報 / 自動付与]
このコンテナに Swift toolchain と SwiftLint は導入されていない。
- インストールを試みてはならない。共有ライブラリ不足で失敗し、時間だけを消費する。
- ビルド・テスト・lint は実行せず、最終出力に「未実行」と明記すること。
- 恒久対応: .vibepod/config.toml の [run] へ profile = "swift" を設定する。

--- ここから利用者のプロンプト ---
```

前置テキストへツールのバージョン番号を含めない。バージョンの正本は `templates/Dockerfile` の ARG であり、前置文へ書き写すと二重管理になる。バージョンが必要な場合、エージェントはコンテナ内で `swift --version` を実行できる。

### 3.5 適用範囲と不変条件

- 前置は `claude_args` にのみ現れる。`RunOptions.prompt` の値は変更しない。
- 次の3経路は前置を含めない: `PromptLock::acquire` のロックキー、`Session.prompt`（`prompt_label` の算出規則を変更しない）、`--verbose` 時のログ表示。
- `opts.prompt` が `None`（対話モード / `--resume`）のとき、`preamble` が `Some` でも `claude_args` に前置は現れない。

前置は環境ごとに決まる定型文であり、セッションの識別と記録にとってはノイズになる。

### 3.6 対話モードの扱い

対話モードは `-p` を使わないため前置経路を持たない。要件1 の起動出力で補う。

## 4. 要件3: `Session` への image / profile 記録

### 4.1 採用する設計

`Session`（`src/session.rs`）へ2フィールドを追加する。

```rust
#[serde(default)]
pub image: Option<String>,
#[serde(default)]
pub profile: Option<String>,
```

### 4.2 格納タイミング

`Session` の構築（`prepare.rs`。`deferred_session`）は `effective_image` の決定より前に位置する。構築位置は変更せず、両フィールドを `None` で初期化し、`effective_image` の決定直後に両方へ代入する。`deferred_session` を `mut` とする。

構築位置を `effective_image` 決定後へ移してはならない。`ensure_image_available` はイメージ未存在時に自動ビルドを行うため、移動すると `started_at` が利用者のコマンド実行時刻から最大でビルド所要時間だけ後ろへずれる。

### 4.3 不変条件

- 両フィールドを持たない既存の `metadata.json` は `#[serde(default)]` により `None` としてデシリアライズできる。既存セッションの読み取り（`vibepod restore` / `logs` / `ps`）を壊さない。
- `image` は要件1 で表示する値と一致する。

起動出力は画面を流れて消えるが、`metadata.json` は残る。事後にどのイメージで実行したかを特定する手段を用意する。

## 5. 要件4: `--help` への profile ポインタ

`vibepod run` の `--lang` ヘルプ文へ次の1行を追記する。

```
Swift toolchain: set `profile = "swift"` in .vibepod/config.toml (see README)
```

`--lang` の選択肢に swift が無いため、help のみを読む利用者が Swift 非対応と誤読する。profile は設定ファイル専用で CLI フラグを持たず、help から到達する経路が他に無い。

## 6. テスト計画

ユニットテスト:

1. `environment_preamble`: 3.3 の3ケースがそれぞれ (a) / (b) / `None` を返す。
2. `build_claude_args`: `preamble` が `Some` かつ `opts.prompt` が `Some` のとき、`-p` の値が前置とプロンプトの結合になる。
3. `build_claude_args`: `preamble` が `None` のとき、`-p` の値がプロンプトそのものになる。
4. `build_claude_args`: `opts.prompt` が `None` のとき、`preamble` が `Some` でも `-p` が現れない。
5. `Session`: `image` / `profile` を持たない JSON をデシリアライズでき、両フィールドが `None` になる。

既存テストの更新:

`build_claude_args` は `pub` であり、`tests/cli_model_flag.rs` が5箇所で2引数の形で呼んでいる。3.2 のシグネチャ変更に伴い、全呼び出しへ `None` を渡す形へ更新する。これらのテストは `--model` の扱いを検証するものであり、期待値は変更しない。

E2E（ホストで実施。ユニットテストの対象外）:

1. `profile = "swift"` のプロジェクトで `vibepod run --prompt` を実行し、起動出力が `Profile: swift (image: vibepod-claude-swift:latest)` を含み、`metadata.json` に両フィールドが記録される。
2. profile 未指定かつ `Package.swift` を持つ検証用リポジトリで、前置 (b) がエージェントへ渡る。
3. profile 未指定かつ `Package.swift` の無いリポジトリで、起動出力が `Profile: default (image: vibepod-claude:latest)` となり、前置が付かない。

## 7. 実装順

1回の Run でまとめて実装する。4要件とも `src/cli/run/prepare.rs` とその周辺で完結し、互いに競合しない。

## 8. 横断更新

- README の profile の節（`#### Swift profile`）へ、要件1 の起動出力例と要件4 のヘルプ文を反映する。
- `docs/design.md` の「CLI 出力イメージ」へ要件1 の `Profile:` 行を反映し、run フローの記述へ要件2 の前置を追加する。
