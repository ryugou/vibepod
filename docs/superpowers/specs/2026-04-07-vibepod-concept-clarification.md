# vibepod コンセプト整理: safety primitive と opinion layer の分離

## Context

vibepod は元々「安全に使える Claude Code」という素朴なコンセプトで始まった。日々の進化の中で機能を足していくうちに、無自覚に 2 つの異なる思想が混ざり込み、新機能設計のたびに「これはどっちで作るか？」がぶれる状態になっていた。

具体的には、以下 2 つの思想が「品質の高いコード」という共通スローガンのもとで同居していた：

- **2a. ホスト環境を持ち込む**: ユーザーがホストで構築した Claude Code 環境（plugins, skills, agents, CLAUDE.md, settings）をそのままコンテナに反映する。立場は「中立 / ユーザー尊重」
- **2b. vibepod が品質を提供する**: vibepod 独自の opinion（推奨 plugin、推奨レビューフロー、推奨プロセス）をデフォルト or 強制で適用する。立場は「opinionated」

例として：

| 機能 | どちら寄りか | 備考 |
|---|---|---|
| Dockerfile での superpowers / frontend-design 焼き込み | 2b | 全ユーザー共通の opinion 押し付け |
| v1.4.3 の `~/.claude/{plugins,skills,agents,CLAUDE.md,settings}` 選択的マウント | 2a | ユーザー環境を反映 |
| `--review codex` フラグ（v1.4 で廃止） | 2b | プロセス強制 |
| `vibepod restore` | 原則1（safety） | 混在しない |
| `~/.codex/auth.json` マウント | 2a | ユーザー認証情報の継承 |

2a と 2b が無自覚に同居していたため、新機能設計時の判断軸が定まらず、過去の機能群にも一貫性がない。

## Decision

vibepod を 2 つの project に切り分け、責務を明確化する。

### Project A: vibepod

役割: **「Claude Code（or 他 runtime）を Docker で安全に動かす」 sandbox primitive**

責務:

- container 隔離 / lifecycle 管理
- mount 管理（workspace, gitconfig, 認証）
- worktree 統合
- `vibepod restore`（git HEAD recovery）
- `vibepod ps` / `logs` / `stop` / `rm`
- `vibepod login` / `logout`
- 言語ツールチェーン自動インストール（`--lang`）

opinion: **ゼロ**

機能追加判断軸: **「safety / sandbox に貢献するか？」**
- Yes → vibepod に入れる
- No → vibepod には入れない（plugin に出す）

### Project B: vibepod plugin (for Claude Code)

役割: **Claude Code から vibepod を呼ぶ正規経路 + 「品質高くコードを書くビルド」 opinion レイヤ**

責務:

- ホストの Claude Code から vibepod を呼ぶ slash command（例: `/vibepod:run`）
- 高品質なコード生成のためのプロセス・規約・テンプレートを内包
- 推奨 plugin set（superpowers, frontend-design など）の宣言
- レビュー・TDD・コミット規約・PR フローなどの opinion を保持
- 内部で vibepod CLI を呼び出して autonomous 実行を委譲

opinion: **これがすべて**

形態: Claude Code plugin（slash command + skill）として実装。ホストの Claude Code に install して使う。

### 役割分離図

```
[ホスト Claude Code]
  ├ install: vibepod plugin (Project B)
  ├ ユーザーの全 plugins / skills / agents が揃ってる
  ├ ブレスト・設計・プロンプト構築
  └ /vibepod:run "..." で投げる
        ↓
  [vibepod container (Project A)]
    ├ 受け取った prompt を autonomous で実行
    ├ vibepod が決めた baseline 環境
    ├ ホストの個別環境は知らない / 持ち込まない
    └ 結果をホストに返す
```

- **Project A** = sandbox（安全）
- **Project B** = opinion（品質）
- 接続点 = `vibepod CLI` の `vibepod run --prompt` を Project B が plugin 経由で呼ぶ

## Implications

この分離から自動的に決まる事項：

1. **vibepod CLI surface の縮小**
   - user-facing: `init`, `login`, `logout`, `ps`, `logs`, `stop`, `rm`, `restore`
   - plugin-oriented (CLI は残すが想定外の直接呼び出し): `run`, `run --prompt`

2. **vibepod に opinion フラグを足さない**
   - 例: `--with-tdd`, `--with-review`, `--auto-format` などの「品質オプション」はすべて Project B 側に置く
   - vibepod の CLI flag は「safety / sandbox の挙動制御」のみ（`--no-network`, `--worktree`, `--mount`, `--lang`, `--new` など）

3. **Dockerfile の plugin 焼き込みを vibepod から外す**
   - 現状 `templates/Dockerfile` で superpowers と frontend-design を焼き込んでいる
   - Project B (plugin) に移管する。具体方法は別途検討

4. **v1.4.3 の host env import は plugin 確定後は論理的に不要**
   - ホスト側で plugin が動く世界では、ホスト Claude Code が user の plugins / skills を使ってプロンプトを組む
   - その上で `/vibepod:run` で vibepod に prompt を投げる
   - vibepod container 内に host plugins を持ち込む必要がない
   - 削除候補（要判断、interactive モードのフォールバックとして残すかどうか）

5. **opinion の進化が vibepod のリリースを巻き込まない**
   - 「TDD やめて型駆動にしたい」のような思想変更は plugin リリースだけで完結
   - vibepod 本体のバージョンは安定して進化できる

6. **誰でも自前の opinion 版を作れる**
   - vibepod は中立 primitive なので、別ユーザーが「自分流 plugin」を作って同じ vibepod 上で動かせる
   - Project B はあくまで「ryugou-style」の 1 例

## 過去機能の棚卸し

本 spec のスコープ外。後日、機能 1 つ 1 つを「Project A 残す / Project B 移管 / 削除」に分類する作業を別途行う。

## スコープ外（本 spec で扱わない）

- 過去機能の具体的な棚卸しと変更計画
- Project B（plugin）の最小 API surface の決定
- v1.4.3 の host env import を実際に削除するかの判断
- Dockerfile からの plugin 焼き込み外しの実装計画
- multi-runtime（Gemini / Codex 等）対応
- v2 dashboard / daemon 化構想
- ホスト側からの通知 / リモート操作 UI
