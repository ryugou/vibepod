//! イメージ未ビルド時の自動ビルド要否判定（純関数）の検証。

use vibepod::cli::init::{auto_build_decision, AutoBuildDecision};

#[test]
fn skip_when_image_exists() {
    assert_eq!(
        auto_build_decision(true, false),
        AutoBuildDecision::Skip,
        "existing image must not trigger a build"
    );
}

#[test]
fn skip_when_image_exists_even_with_no_auto_build() {
    // --no-auto-build はイメージが無いときだけ意味を持つ。
    assert_eq!(auto_build_decision(true, true), AutoBuildDecision::Skip);
}

#[test]
fn build_when_missing_and_allowed() {
    assert_eq!(
        auto_build_decision(false, false),
        AutoBuildDecision::Build,
        "missing image with auto-build allowed must build"
    );
}

#[test]
fn fail_when_missing_and_no_auto_build() {
    assert_eq!(
        auto_build_decision(false, true),
        AutoBuildDecision::FailNoAutoBuild,
        "missing image with --no-auto-build must fail fast"
    );
}
