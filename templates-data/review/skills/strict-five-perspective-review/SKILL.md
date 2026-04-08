---
name: strict-five-perspective-review
description: Security / Reliability / Performance / Maintainability / Architecture の 5 観点を機械的に適用する。深掘り不足による「問題なし」を禁じるが、誠実な深掘り後の空 findings は PASS として許容する。use when コードをレビューするとき
---

# Strict Five-Perspective Review

コードレビューは以下の 5 観点を **機械的に全部回す**。1 つでも飛ばさない。

## 観点 1: Security

以下を順にチェックする:

1. **入力検証**: 外部入力 (user input / HTTP body / file / env var / CLI args) が検証されているか
   - SQL: parameterized query になっているか（string concat 禁止）
   - Shell: `subprocess` / `Command` で user input を渡していないか、エスケープされているか
   - Path: `../` / absolute path を拒否しているか、canonicalize + in-root チェックがあるか
   - Deserialization: 信頼できないデータを直接 deserialize していないか
2. **秘密情報**: ハードコードされた password / API key / token / private key が無いか
3. **認証・認可**: API エンドポイント / 操作権限のチェックが漏れていないか、権限昇格の余地が無いか
4. **暗号化**: MD5 / SHA1 / ECB mode / hard-coded IV 等の弱い選択が無いか、salt が付いているか
5. **依存脆弱性**: 追加された crate / npm / pip に既知の脆弱性がないか
6. **unsafe / FFI**: `unsafe` ブロックの不変条件が正しいか、FFI 境界での memory safety

## 観点 2: Reliability

1. **エラーハンドリング**: 全エラーパスが処理されているか
   - `unwrap()` / `expect()` がプロダクションコードに無いか
   - `Result` を `_` で捨てていないか
   - `?` で伝播した error に context が付いているか
2. **Panic の可能性**: slice indexing `arr[i]`, 0 除算, arithmetic overflow
3. **並行性**: race condition、deadlock、lock 保持中の長時間処理
4. **リソースリーク**: file / socket / lock / thread が drop されるか
5. **部分失敗**: transaction 的操作の途中失敗で state が壊れないか
6. **Retry / timeout**: 外部呼び出しに timeout があるか、失敗時の挙動が明確か

## 観点 3: Performance

1. **Allocation**: 不要な `clone()` / `to_string()` / `to_vec()` / `collect()`
2. **計算量**: ネストループで O(n^2) / O(n*m) になっていないか
3. **DB アクセス**: N+1 / ループ内で query / bulk 化漏れ
4. **async**: `.await` 境界で block している IO (`std::fs` / `std::net`) 呼び出しが無いか
5. **Stack**: 巨大な値の stack allocation / move コスト
6. **キャッシュ/メモ化**: 同じ計算を繰り返していないか

## 観点 4: Maintainability

1. **命名**: 短縮・略語・汎用名 (`data` / `info` / `tmp`) が意図を隠していないか
2. **関数サイズ**: 50 行以上は分割検討、100 行超は必須分割
3. **型の責務**: 1 型 1 責務か、God object になっていないか
4. **コメント**: 非自明な所にコメントがあるか、自明な所に不要なコメントが無いか
5. **テスト**: 変更箇所に対応するテストがあるか、内部実装に結合していないか
6. **Duplicate**: 3 回目の重複があれば抽象化、2 回目までは許容
7. **Dead code**: 使われていない import / 関数 / 型 / 変数が無いか

## 観点 5: Architecture

1. **Layer**: 下位層が上位層を参照していないか
2. **Abstraction**: premature な trait / generic が無いか、逆に足りない境界が無いか
3. **Module 境界**: 循環依存が無いか、境界が曖昧になっていないか
4. **拡張点**: 将来の拡張が必要な所に余地があるか / 逆に過剰な拡張点が無いか
5. **一貫性**: 既存 pattern (naming / error handling / logging) に従っているか
6. **分離**: 副作用 (IO / global state) が境界に寄せられているか

## 重要度分類

指摘事項は以下の 3 段階で分類する:

### Critical

- Security の脆弱性
- データ破壊・データ損失の可能性
- 認証・認可の漏れ
- プロダクション crash の確実な誘発
- 既存機能の破壊

→ **1 件でもあれば FAIL**

### Warning

- エラーハンドリングの欠落（クラッシュに繋がる）
- race condition
- 顕著なパフォーマンス劣化
- 大幅な保守性の劣化（100 行超関数、God object 化）
- 設計一貫性の明らかな破壊

→ **原則修正後 merge**、CONDITIONAL PASS

### Suggestion

- 小規模な命名改善
- より慣用的な書き方の提案
- テスト追加の提案
- コメント追加の提案
- 軽微なリファクタリング案

→ **merge 可**、PASS

## 指摘の誠実さ

- **本当にクリーンな diff は `PASS` で findings 空にしてよい**。
  無理やり Suggestion を捻り出して false positive を作ると、review
  template の信頼性そのものを壊す。
- ただしそこに到達するには以下を **すべて満たす** 必要がある:
  1. 5 観点を機械的に全て回した (カバレッジ checklist が `[x][x][x][x][x]`)
  2. 変更されたファイルを **全部 Read した** (diff 要約ではなく本文)
  3. 各観点ごとに「この diff で該当するか」を 1 つ以上考察した
  4. それでも Critical / Warning / Suggestion のいずれも根拠ある形で
     出せない
- 実際には人間が書いた現実のコードで 1 つも改善余地が無いことは稀。
  「見つからない」と感じたら自分の観点の深さを疑い、もう一段深掘り
  する (pattern の一貫性、命名、テストカバレッジ、コメントの過不足、
  依存の追加・削除が無いか、等)。
- **諦めの「問題なし」は禁止**。深掘り不足の結果としての「空 findings」
  は虚偽。報告は「深掘りした結果空だった」でなければならない。

## 禁則

- 「特に問題ありません」で終わる
- 「たぶん / おそらく / 少し気になる」などの曖昧表現
- ファイル名・行番号の無い抽象指摘
- 改善案の無い批判のみの指摘
- Critical を「怖いから Warning に」降格する
- Warning を「面倒だから Suggestion に」降格する
