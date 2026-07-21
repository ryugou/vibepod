# v1.8 作業中に発見した既知バグ(未修正・別対応)

codex-in-container の実装・レビュー中に発見した、本ブランチのスコープ外のバグ。

## 1. `vibepod run --resume` が Claude Code 2.1.216 以降で機能しない

`claude -p --resume` はセッション ID または title が必須になった
(`Error: --resume requires a valid session ID or session title when used with --print`)が、
vibepod は引数なしの `--resume` を渡している。直近セッション ID を
`.vibepod/sessions/` のメタデータから解決して渡す修正が必要。

## 2. idle タイムアウトの既定 5 分がコンテナ内コールドビルドに対して短すぎる

コンテナ内の `cargo build`(linux 向けコールドビルド)は 5 分以上無出力になり得るため、
既定値のままだと正常な実装セッションが「ストリーム無出力」で打ち切られる。
`~/.config/vibepod/config.toml` の `[run] prompt_idle_timeout` で回避可能だが、
既定値の引き上げ(例: 15分)または「サブプロセス実行中は idle 判定を緩める」対応を検討する。
