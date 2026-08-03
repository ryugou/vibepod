//! `init --rebuild` 時に swift バリアントイメージを再ビルドするかどうかの
//! 判定（純関数）の検証。docker を呼ばずに 4 通りの組み合わせを網羅する。
//!
//! 特に重要なのは「引数無し `vibepod init`（rebuild=false）では、swift
//! イメージが存在していても再ビルドしない」という不変条件で、これが
//! 破られると未使用のプロファイルを勝手にビルドし始めてしまう。

use vibepod::cli::init::swift_rebuild_decision;

#[test]
fn no_rebuild_when_flag_not_set_and_swift_image_exists() {
    // 重要な不変条件: 引数無し `vibepod init` では swift をビルドしない。
    assert!(
        !swift_rebuild_decision(false, true),
        "swift image existing must not trigger a rebuild without --rebuild"
    );
}

#[test]
fn no_rebuild_when_flag_not_set_and_swift_image_missing() {
    assert!(!swift_rebuild_decision(false, false));
}

#[test]
fn rebuild_when_flag_set_and_swift_image_exists() {
    assert!(
        swift_rebuild_decision(true, true),
        "an existing swift image must be rebuilt alongside the default image under --rebuild"
    );
}

#[test]
fn no_rebuild_when_flag_set_but_swift_image_missing() {
    // --rebuild が付いていても、swift イメージを一度も作っていない環境では
    // 未使用のバリアントを勝手にビルドし始めない。
    assert!(!swift_rebuild_decision(true, false));
}
