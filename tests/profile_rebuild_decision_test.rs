//! `init --rebuild` 時に profile バリアントイメージを再ビルドするかどうかの
//! 判定（純関数）の検証。docker を呼ばずに 4 通りの組み合わせを網羅する。
//!
//! 特に重要なのは「引数無し `vibepod init`（rebuild=false）では、profile
//! イメージが存在していても再ビルドしない」という不変条件で、これが
//! 破られると未使用のプロファイルを勝手にビルドし始めてしまう。
//!
//! F7（フル再レビュー指摘）: このファイル・関数はもともと `swift` 専用の
//! 名前（`swift_rebuild_decision` / `tests/swift_rebuild_decision_test.rs`）
//! だったが、判定ロジック自体は元から profile 名に依存しない
//! （`rebuild && exists` のみ）。呼び出し側（`src/cli/init.rs`）を
//! `config::VALID_PROFILES` のループへ一般化したのに合わせ、profile 非依存の
//! 名前 `profile_rebuild_decision` へリネームした。

use vibepod::cli::init::profile_rebuild_decision;

#[test]
fn no_rebuild_when_flag_not_set_and_profile_image_exists() {
    // 重要な不変条件: 引数無し `vibepod init` では profile イメージをビルドしない。
    assert!(
        !profile_rebuild_decision(false, true),
        "profile image existing must not trigger a rebuild without --rebuild"
    );
}

#[test]
fn no_rebuild_when_flag_not_set_and_profile_image_missing() {
    assert!(!profile_rebuild_decision(false, false));
}

#[test]
fn rebuild_when_flag_set_and_profile_image_exists() {
    assert!(
        profile_rebuild_decision(true, true),
        "an existing profile image must be rebuilt alongside the default image under --rebuild"
    );
}

#[test]
fn no_rebuild_when_flag_set_but_profile_image_missing() {
    // --rebuild が付いていても、profile イメージを一度も作っていない環境では
    // 未使用のバリアントを勝手にビルドし始めない。
    assert!(!profile_rebuild_decision(true, false));
}
