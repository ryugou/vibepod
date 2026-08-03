//! Static regression guard for `templates/Dockerfile`'s profile/distro stage
//! wiring, and for the Swift/SwiftLint version pins (reviewer round,
//! 2026-08-03, findings W2 / S4).
//!
//! Background (W2): the Dockerfile was restructured from a single
//! `debian:bookworm-slim` base into a `distro-<profile>` / `profile-<profile>`
//! stage pair per profile (`FROM distro-${VIBEPOD_PROFILE} AS base` ...
//! `FROM profile-${VIBEPOD_PROFILE}`) — see 2.3.1 in
//! `docs/superpowers/specs/2026-08-03-swift-profile-and-session-hardening-design.md`
//! for why `swift` needs `debian:trixie-slim` while `default` stays on
//! `debian:bookworm-slim`. Nothing in the Rust type system enforces that
//! every entry in `config::VALID_PROFILES` has a matching pair of Dockerfile
//! stages: a future profile added to `VALID_PROFILES` without updating the
//! Dockerfile would compile and pass `cargo test` fine, and only fail at
//! `docker build --build-arg VIBEPOD_PROFILE=<profile>` time with a
//! "stage not found" error deep in a multi-minute build. This test reads
//! `VALID_PROFILES` directly and asserts the pairing statically so that
//! mismatch is caught by `cargo test` (seconds) instead of a real build
//! (minutes).
//!
//! Background (S4): the Swift toolchain and SwiftLint provisioning steps
//! follow the same "pinned version + per-arch SHA256 table, verified before
//! extraction" pattern as the codex CLI (see `dockerfile_codex_pin_test.rs`),
//! but unlike codex they intentionally have NO `latest` escape hatch — a
//! Swift/SwiftLint version bump must always add verified checksums, no
//! exceptions. These tests guard the parts of that contract that are simple,
//! non-brittle text assertions (pinned default, checksum table entries, no
//! escape-hatch branch).
//!
//! This environment has no `docker` binary, so these are text-based
//! structural assertions on `templates/Dockerfile`, not real builds. They
//! intentionally avoid pinning to exact shell formatting so minor
//! rewording/reflow doesn't make this test brittle.

mod common;

use common::read_dockerfile;
use vibepod::config::VALID_PROFILES;

/// SHA256 for `swift-6.3.3-RELEASE-debian12-aarch64.tar.gz`, verified
/// out-of-band and recorded in `templates/Dockerfile`'s checksum table.
const SWIFT_AARCH64_SHA256: &str =
    "ecba8ef87b54a5048d466af500f3169c939a6b8a2cb7c600f76b5184457f293a";

/// SHA256 for `swift-6.3.3-RELEASE-debian12.tar.gz` (x86_64), same source.
const SWIFT_X86_64_SHA256: &str =
    "19e0c78cad5418ad48bfa87aa20c53ac9ac9996d1695d04dd94f7c7ea4eb133f";

/// SHA256 for `swiftlint_linux_arm64.zip` at 0.65.0.
const SWIFTLINT_ARM64_SHA256: &str =
    "12d3b84bc5b69ae13a99a5a5c79904f9ce25867f099f6368d0037854f9ee6c26";

/// SHA256 for `swiftlint_linux_amd64.zip` at 0.65.0.
const SWIFTLINT_AMD64_SHA256: &str =
    "79306a34e5c7cc55a220cd108cbb861dcad5f10138dcdf261e2624ae8b0a486b";

/// `dockerfile` が `AS <stage_name>` を、Docker のステージ名として妥当な
/// トークン境界で含むかを判定する。
///
/// 単純な `str::contains(&format!("AS {stage_name}"))` だと、例えば
/// `stage_name = "distro-swift"` は `AS distro-swift-experimental` のような
/// 別ステージ名にも部分一致してしまい、意図したステージが実在しなくても
/// テストが green になってしまう（実際にこの関数を書く前に手元で踏んだ）。
/// マッチ直後の文字が Docker のステージ名に使える文字
/// （英数字・`_`・`.`・`-`）でない、または文字列末尾であることまで確認する。
fn dockerfile_has_stage(dockerfile: &str, stage_name: &str) -> bool {
    let needle = format!("AS {stage_name}");
    let mut search_start = 0;
    while let Some(rel_idx) = dockerfile[search_start..].find(&needle) {
        let idx = search_start + rel_idx;
        let after = idx + needle.len();
        let is_boundary = match dockerfile[after..].chars().next() {
            None => true,
            Some(c) => !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'),
        };
        if is_boundary {
            return true;
        }
        // 部分一致だった場合、同じ位置から再検索すると無限ループするため
        // 1 文字進めてから続ける。
        search_start = idx + 1;
    }
    false
}

#[test]
fn each_valid_profile_has_a_distro_and_profile_stage() {
    let dockerfile = read_dockerfile();
    for profile in VALID_PROFILES {
        let distro_stage = format!("distro-{profile}");
        assert!(
            dockerfile_has_stage(&dockerfile, &distro_stage),
            "config::VALID_PROFILES contains \"{profile}\" but templates/Dockerfile has no \
             `AS {distro_stage}` stage. Every valid profile needs its own distro alias stage \
             (`FROM <image> AS distro-<profile>`), even one that just reuses \
             `debian:bookworm-slim` like `distro-default` does. Without it, \
             `FROM distro-${{VIBEPOD_PROFILE}} AS base` resolves to a nonexistent stage and \
             `docker build --build-arg VIBEPOD_PROFILE={profile}` fails at build time instead of \
             here at `cargo test` time."
        );

        let profile_stage = format!("profile-{profile}");
        assert!(
            dockerfile_has_stage(&dockerfile, &profile_stage),
            "config::VALID_PROFILES contains \"{profile}\" but templates/Dockerfile has no \
             `AS {profile_stage}` stage. Every valid profile needs a `FROM base AS profile-<profile>` \
             stage so the final `FROM profile-${{VIBEPOD_PROFILE}}` can resolve."
        );
    }
}

#[test]
fn swift_version_defaults_to_a_pinned_release() {
    let dockerfile = read_dockerfile();
    assert!(
        dockerfile.contains("ARG SWIFT_VERSION=6.3.3"),
        "expected `ARG SWIFT_VERSION=6.3.3` (a pinned default) in templates/Dockerfile so a \
         plain `docker build` reproducibly installs a known Swift toolchain version"
    );
}

#[test]
fn swift_checksum_table_contains_both_verified_arch_hashes() {
    let dockerfile = read_dockerfile();
    assert!(
        dockerfile.contains(SWIFT_AARCH64_SHA256),
        "expected the verified aarch64 SHA256 ({SWIFT_AARCH64_SHA256}) for Swift 6.3.3 to be \
         present in templates/Dockerfile's checksum table"
    );
    assert!(
        dockerfile.contains(SWIFT_X86_64_SHA256),
        "expected the verified x86_64 SHA256 ({SWIFT_X86_64_SHA256}) for Swift 6.3.3 to be \
         present in templates/Dockerfile's checksum table"
    );
}

#[test]
fn swiftlint_version_defaults_to_a_pinned_release() {
    let dockerfile = read_dockerfile();
    assert!(
        dockerfile.contains("ARG SWIFTLINT_VERSION=0.65.0"),
        "expected `ARG SWIFTLINT_VERSION=0.65.0` (a pinned default) in templates/Dockerfile"
    );
}

#[test]
fn swiftlint_checksum_table_contains_both_verified_arch_hashes() {
    let dockerfile = read_dockerfile();
    assert!(
        dockerfile.contains(SWIFTLINT_ARM64_SHA256),
        "expected the verified arm64 SHA256 ({SWIFTLINT_ARM64_SHA256}) for SwiftLint 0.65.0 to \
         be present in templates/Dockerfile's checksum table"
    );
    assert!(
        dockerfile.contains(SWIFTLINT_AMD64_SHA256),
        "expected the verified amd64 SHA256 ({SWIFTLINT_AMD64_SHA256}) for SwiftLint 0.65.0 to \
         be present in templates/Dockerfile's checksum table"
    );
}

#[test]
fn swift_and_swiftlint_provisioning_has_no_latest_escape_hatch() {
    // 対象範囲: Swift toolchain の ARG 宣言から、スモークチェック
    // (`RUN swift --version && swiftlint version`) までの、両バージョン
    // 引数を扱う provisioning ブロック全体。
    let dockerfile = read_dockerfile();
    let start = dockerfile
        .find("ARG SWIFT_VERSION")
        .expect("expected `ARG SWIFT_VERSION` in templates/Dockerfile (checked above)");
    let end = dockerfile
        .find("RUN swift --version && swiftlint version")
        .expect("expected the swift/swiftlint smoke check `RUN swift --version && swiftlint version` in templates/Dockerfile");
    let block = &dockerfile[start..end];

    // codex の `if [ "$CODEX_VERSION" = "latest" ]; then ... else ... fi` に
    // 相当する分岐が無いことを、その分岐でのみ現れる正確なトークン
    // (`"latest"` に引用符が付いた形)で判定する。素の "latest" という単語
    // だけで判定すると、「latest エスケープハッチは無し」という日本語
    // コメント自体に "latest" という語が含まれるため誤検知する。
    assert!(
        !block.contains("\"latest\""),
        "expected no `= \"latest\"`-style version escape hatch in the Swift/SwiftLint \
         provisioning block (unlike codex's CODEX_VERSION=latest path, a Swift/SwiftLint \
         version bump must always add verified checksums — no unverified fallback): {block}"
    );
}
