# review モードを deny-by-default（Bash(*) deny + 読み取り allow）へ反転する

## 課題（ステータス: 起票のみ / 未実装）

現在の `--mode review` は **allow-by-default + 個別 deny** で書き込みを塞いでいる。
`templates-data/<lang>/review/settings.json` の `permissions.deny` に
`Edit(*)` / `Write(*)` / `NotebookEdit(*)` と、破壊系の `rm` / `mv` / 履歴改変系
`git` サブコマンドを列挙する方式である。

この方式は **主要な書き込み経路は塞ぐが、完全なサンドボックスではない**。
`Bash` そのものは許可されたままなので、deny リストに載っていない経路での
書き込みが素通りする:

- `tee`（`echo x | tee file`）
- リダイレクト（`> file` / `>> file`）
- `sed -i` / `perl -i` などのインプレース編集
- その他 deny パターンに一致しないシェル経由の任意書き込み

現状の安全性の最終的な担保は「コンテナ境界（ホストFSへ書けない）」と「git で
差し戻せること」であり、review モードの `permissions.deny` は
**あくまで直接的な編集・コミット経路のガードレール**という位置づけである。

## あるべき姿（v1.8 の実装方針）

`permissions` を **deny-by-default** に反転する:

1. `Bash(*)` を deny する（既定で全シェルコマンドを拒否）。
2. レビューに必要な**読み取り専用コマンドのみ** allow リストで開ける
   （例: `Bash(cat:*)` / `Bash(rg:*)` / `Bash(grep:*)` / `Bash(git log:*)` /
   `Bash(git diff:*)` / `Bash(git show:*)` / `Bash(ls:*)` / `Bash(find:*)` 等）。
3. `Edit` / `Write` / `NotebookEdit` は引き続き deny。

これにより「明示的に許可された読み取りコマンド以外は一切実行できない」状態になり、
`tee` / リダイレクト / `sed -i` を含む任意書き込みが原理的に塞がれる。

## 実装上の論点（着手時に詰めること）

- **allow リストの網羅性 vs 使い勝手**: 絞りすぎるとレビューに必要な調査コマンド
  （ビルド確認、テスト実行など）が打てず有用性が落ちる。read-only と
  「副作用はあるが安全」の線引きを言語バンドルごとに決める必要がある。
- **リダイレクトの扱い**: `Bash(cat:*)` を許可しても `cat x > y` のリダイレクトが
  Claude Code の permission マッチングでどう評価されるかを実機検証する
  （コマンド名マッチングがリダイレクト部分を無視するなら allow が抜け穴になる）。
- **6 バンドル（generic/go/java/node/python/rust）の review settings.json を
  一括更新**する。`tests/cli_review_permissions.rs` に「`Bash(*)` が deny に
  含まれること」を固定するテストを追加する。
- 既存の `enabledPlugins: {}` 不変条件（v1.7 で固定済み）は維持する。

## 関連

- v1.7: host `~/.claude/` を全モードでマウント、review の安全性論拠を
  template 側 `permissions.deny` に置く方針を明文化。本課題はその論拠を
  「ガードレール」から「サンドボックス」へ引き上げるもの。
- README の "Review mode (read-only evaluation)" 節に、現状が完全な
  サンドボックスではない旨と本課題への参照を記載済み。
