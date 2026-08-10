//! `environment_preamble` は profile と workspace の状態から、コンテナ内
//! エージェントへ渡す環境情報ブロックを導出する純関数。
//! 設計: docs/superpowers/specs/2026-08-10-profile-visibility-and-environment-disclosure-design.md 3.3

use vibepod::cli::run::prepare::environment_preamble;

const PROMPT_DELIMITER: &str = "--- ここから利用者のプロンプト ---";

#[test]
fn swift_profile_returns_available_preamble() {
    let p =
        environment_preamble(Some("swift"), false).expect("swift profile must produce a preamble");
    assert!(p.contains("導入済み"), "got: {p}");
    assert!(
        p.ends_with(PROMPT_DELIMITER),
        "preamble must end with the delimiter; got: {p}"
    );
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
    let p = environment_preamble(None, true)
        .expect("Package.swift without profile must produce a preamble");
    assert!(p.contains("導入されていない"), "got: {p}");
    assert!(
        p.contains("profile = \"swift\""),
        "must point to the fix; got: {p}"
    );
    assert!(
        p.ends_with(PROMPT_DELIMITER),
        "preamble must end with the delimiter; got: {p}"
    );
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
