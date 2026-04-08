---
name: review-report-formatter
description: レビュー結果を strict な所定フォーマットで出力する。判定・Summary・Critical/Warning/Suggestion セクション、各 issue にファイル行番号と改善案。use when レビュー結果をユーザーに返すとき
---

# Review Report Formatter

レビュー結果は必ず以下のフォーマットで出力する。セクションを飛ばさない、順序を変えない、追加の雑談を挟まない。

## フォーマット

```markdown
# Review Report

**判定: {PASS | CONDITIONAL PASS | FAIL}**

## Summary

{1-3 文で全体所見。何を review したか、主要な観察点}

## Critical Issues

{Critical が無ければ「なし」と明記}

### C-1. {簡潔なタイトル}

- **ファイル**: `path/to/file.rs:123-145`
- **観点**: {Security | Reliability | Performance | Maintainability | Architecture}
- **問題**:
  {具体的に何が問題か、なぜ Critical か}
- **該当コード**:
  ```rust
  // 該当箇所の抜粋
  ```
- **改善案**:
  {具体的な修正方針、または修正後のコード例}

### C-2. ...

## Warnings

{Warning が無ければ「なし」と明記}

### W-1. {簡潔なタイトル}

- **ファイル**: `path/to/file.rs:67`
- **観点**: {...}
- **問題**: ...
- **該当コード**: ...
- **改善案**: ...

### W-2. ...

## Suggestions

### S-1. {簡潔なタイトル}

- **ファイル**: `path/to/file.rs:42`
- **観点**: {...}
- **問題**: ...
- **該当コード**: ...
- **改善案**: ...

### S-2. ...

## 観点カバレッジ

以下 5 観点を機械的に適用した:

- [x] Security
- [x] Reliability
- [x] Performance
- [x] Maintainability
- [x] Architecture
```

## ルール

1. **判定は最初に出す**。PASS / CONDITIONAL PASS / FAIL のどれか 1 つ。「部分的に PASS」のような曖昧な表現は禁止。

2. **Summary は 1-3 文**。雑談・挨拶・前置き禁止。「〜について review しました」のような redundant な書き出しも禁止。

3. **各 issue は必ず以下 5 要素を含む**:
   - タイトル（短く）
   - ファイル:行番号
   - 観点（5 つのどれか）
   - 問題説明
   - 該当コード抜粋
   - 改善案

4. **該当コード** は diff の該当箇所をそのまま貼る。抜粋は長すぎず短すぎず、問題の根拠が分かる範囲で。

5. **改善案** はコード例 or 明確な方針。抽象論で終わらない。例:
   - 悪い: 「エラーハンドリングを改善する」
   - 良い: 「L123 の `unwrap()` を `?` に変え、呼び出し側の戻り値を `Result<(), anyhow::Error>` にする。context は `with_context(|| format!("failed to parse {}", path.display()))` を追加」

6. **Critical / Warning が無く、Suggestions も深掘りの結果ゼロ** だった
   場合は、`Suggestions` セクションに「なし (5 観点を機械的に回した結果、
   誠実に指摘できる改善点は検出されませんでした)」と明記する。
   false positive を作って埋めない。ただしこれは `strict-five-perspective-review`
   skill の「指摘の誠実さ」節の条件を **全部** 満たした場合のみ許される。

7. **観点カバレッジ** の checkbox は 5 つ全部 `[x]` になっているはず。1 つでも飛ばしていたらレビューが不十分。

## 判定のルール

- **FAIL**: Critical が 1 件以上ある
- **CONDITIONAL PASS**: Critical なし、Warning が 1 件以上ある
- **PASS**: Critical / Warning なし、Suggestion のみ

これ以外の判定は無い。

## 禁則

- セクションを省略する
- フォーマット外の雑談・結論を先に書く
- 「以下、review 結果です」などの無駄な前置き
- 絵文字・カラー文字・強調装飾の濫用（必要な `**太字**` は OK）
- ファイル名・行番号の無い指摘
- 改善案の無い指摘
- 「問題なし」だけの Summary
