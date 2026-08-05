//! Static regression guard for `templates/Dockerfile`'s profile/distro stage
//! wiring, and for the Swift/SwiftLint version pins (reviewer round,
//! 2026-08-03, findings W2 / S4; full re-review round, findings F4 / F5 / F6).
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
//! Background (F4): `"default"` is never in `VALID_PROFILES` (it means
//! "profile unset", not a selectable value — see `RunConfig::profile` doc),
//! so the loop above never actually exercises the `distro-default` /
//! `profile-default` stages or the two `${VIBEPOD_PROFILE}` selector lines
//! that pick between them at build time. Those are asserted explicitly here.
//!
//! Background (S4/F6): the Swift toolchain and SwiftLint provisioning steps
//! follow the same "pinned version + per-arch SHA256 table, verified before
//! extraction" pattern as the codex CLI (see `dockerfile_codex_pin_test.rs`),
//! but unlike codex they intentionally have NO `latest` escape hatch — a
//! Swift/SwiftLint version bump must always add verified checksums, no
//! exceptions. F6 tightened the checksum-table assertions to require the
//! hashes appear specifically inside the provisioning RUN block (not merely
//! somewhere in the file), reusing the same block extraction as the
//! no-latest-escape-hatch test below.
//!
//! Background (F5): `dockerfile_has_stage` used to search the Dockerfile's
//! full text for `AS <stage_name>`, which means a comment merely *mentioning*
//! a stage name (e.g. `# see AS distro-ghost for context`) would count as
//! that stage existing. Rewritten to only match `FROM` lines (comment lines
//! excluded), with a regression test proving the old behavior would have
//! been fooled.
//!
//! This environment has no `docker` binary, so these are text-based
//! structural assertions on `templates/Dockerfile`, not real builds. They
//! intentionally avoid pinning to exact shell formatting so minor
//! rewording/reflow doesn't make this test brittle.

mod common;

use common::read_dockerfile;
use vibepod::config::VALID_PROFILES;

/// SHA256 for `swift-6.3.3-RELEASE-debian12-aarch64.tar.gz`.
///
/// 検証手段・日付（F6）: 2026-08-03、swift.org からの実ダウンロード +
/// `sha256sum` で計算し確認済み。さらに同日、`VIBEPOD_PROFILE=swift` で実際に
/// `docker build` を実行し、Dockerfile 内の `sha256sum -c` ステップがこの
/// アセットに対して `: OK` を出力することも確認済み（ビルドホストが
/// aarch64 だったため、この経路で実機検証できたのは aarch64 のみ）。
const SWIFT_AARCH64_SHA256: &str =
    "ecba8ef87b54a5048d466af500f3169c939a6b8a2cb7c600f76b5184457f293a";

/// SHA256 for `swift-6.3.3-RELEASE-debian12.tar.gz` (x86_64)。
///
/// 検証手段・日付（F6）: 2026-08-03、swift.org からの実ダウンロード +
/// `sha256sum` で計算し確認済み。x86_64 ビルドホストが無いため、
/// `docker build` 経由での実機再検証は行っていない（ダウンロード +
/// ハッシュ計算のみ）。
const SWIFT_X86_64_SHA256: &str =
    "19e0c78cad5418ad48bfa87aa20c53ac9ac9996d1695d04dd94f7c7ea4eb133f";

/// SHA256 for `swiftlint_linux_arm64.zip` at 0.65.0。
///
/// 検証手段・日付（F6）: 2026-08-03、GitHub Releases からの実ダウンロード +
/// `sha256sum` で計算し確認済み。同日の `docker build`（aarch64）で
/// `swiftlint version` が `0.65.0` を出力することも確認済み。
const SWIFTLINT_ARM64_SHA256: &str =
    "12d3b84bc5b69ae13a99a5a5c79904f9ce25867f099f6368d0037854f9ee6c26";

/// SHA256 for `swiftlint_linux_amd64.zip` at 0.65.0。
///
/// 検証手段・日付（F6）: 2026-08-03、GitHub Releases からの実ダウンロード +
/// `sha256sum` で計算し確認済み。amd64 ビルドホストが無いため、
/// `docker build` 経由での実機再検証は行っていない（ダウンロード +
/// ハッシュ計算のみ）。
const SWIFTLINT_AMD64_SHA256: &str =
    "79306a34e5c7cc55a220cd108cbb861dcad5f10138dcdf261e2624ae8b0a486b";

/// `dockerfile` の `FROM ... AS <stage_name>` 行に、`AS <stage_name>` が
/// Docker のステージ名として妥当なトークン境界で現れるかを判定する。
///
/// F5（フル再レビュー指摘）: 以前は全文検索（`FROM` 行かどうかを見ない）
/// だったため、コメント行に `AS distro-ghost` という文字列が書かれている
/// だけで green になり、対応する実際のステージ宣言が無くても検知できな
/// かった（`dockerfile_has_stage_ignores_comment_mentions` がこの回帰を
/// 固定する）。`FROM` で始まる行のみを対象にすることで、コメント・
/// ドキュメント文字列中の言及と実際のステージ宣言を区別する。
///
/// トークン境界の判定自体は元の実装を踏襲する: マッチ直後の文字が Docker の
/// ステージ名に使える文字（英数字・`_`・`.`・`-`）でない、または行末である
/// ことまで確認する（`AS distro-swift` が `AS distro-swift-experimental` に
/// 部分一致するのを防ぐ）。
fn dockerfile_has_stage(dockerfile: &str, stage_name: &str) -> bool {
    let needle = format!("AS {stage_name}");
    for line in dockerfile.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') || !trimmed.starts_with("FROM ") {
            continue;
        }
        let Some(idx) = line.find(&needle) else {
            continue;
        };
        let after = idx + needle.len();
        let is_boundary = match line[after..].chars().next() {
            None => true,
            Some(c) => !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-'),
        };
        if is_boundary {
            return true;
        }
    }
    false
}

/// Swift toolchain の `ARG SWIFT_VERSION` 宣言から、スモークチェック
/// (`RUN bash --login -c 'swift --version && swiftlint version'`) 直前
/// までの provisioning ブロックを切り出す。
///
/// F6（フル再レビュー指摘）: バージョン pin・SHA256 テーブルの実在チェックを
/// ファイル全文への `contains` にすると、たとえば別セクションに同じ文字列が
/// 偶然/コメントとして書かれているだけでも green になり得る。この関数で
/// 切り出した provisioning ブロック内に限定してチェックすることで、
/// 「実際に使われている pin・ハッシュか」を確認する。
///
/// 実欠陥修正（E2E スモークで発覚: `bash --login` 起動だと `/etc/profile`
/// が PATH をリセットし `ENV PATH` だけでは login シェルで `swift` が
/// 見つからない）に伴い、スモークチェックを login シェル経由
/// （`RUN bash --login -c '...'`）に変更したため、終端マーカーもそれに
/// 合わせて更新した。
fn provisioning_block(dockerfile: &str) -> &str {
    let start = dockerfile
        .find("ARG SWIFT_VERSION")
        .expect("expected `ARG SWIFT_VERSION` in templates/Dockerfile");
    let end = dockerfile
        .find("RUN bash --login -c 'swift --version && swiftlint version'")
        .expect(
            "expected the swift/swiftlint smoke check \
             `RUN bash --login -c 'swift --version && swiftlint version'` in templates/Dockerfile",
        );
    &dockerfile[start..end]
}

#[test]
fn dockerfile_has_stage_ignores_comment_mentions() {
    // F5（フル再レビュー指摘）回帰: 全文検索ベースの旧実装は、コメント行に
    // `AS distro-ghost` という文字列が書かれているだけで green になり、
    // 実際には対応する `FROM ... AS distro-ghost` 宣言が無くても検知できな
    // かった。`FROM` 行のみを対象にすることで、コメント中の言及と実際の
    // ステージ宣言を区別する。
    let synthetic =
        "# see AS distro-ghost for context\nFROM debian:bookworm-slim AS distro-default\n";
    assert!(
        !dockerfile_has_stage(synthetic, "distro-ghost"),
        "a comment merely mentioning `AS distro-ghost` must not count as a real stage"
    );
    assert!(
        dockerfile_has_stage(synthetic, "distro-default"),
        "an actual `FROM ... AS distro-default` line must still be detected"
    );
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
fn default_profile_has_a_distro_and_profile_stage() {
    // F4（フル再レビュー指摘）: `"default"` は `VALID_PROFILES` に含まれない
    // （profile 未指定を表す特別扱いのため — `RunConfig::profile` の doc
    // 参照）ので、上のループでは `distro-default` / `profile-default` が
    // 一度も検証されない。明示的に固定する。
    let dockerfile = read_dockerfile();
    assert!(
        dockerfile_has_stage(&dockerfile, "distro-default"),
        "expected `AS distro-default` stage in templates/Dockerfile (the fallback distro for \
         unset/invalid VIBEPOD_PROFILE build args)"
    );
    assert!(
        dockerfile_has_stage(&dockerfile, "profile-default"),
        "expected `AS profile-default` stage in templates/Dockerfile"
    );
}

#[test]
fn dynamic_profile_selectors_are_present() {
    // F4（フル再レビュー指摘）: `distro-<profile>` / `profile-<profile>` の
    // 個々のステージが揃っていても、それらを実際に選択する2本の動的セレクタ
    // 行（`ARG VIBEPOD_PROFILE` を式展開する `FROM` 行）が無ければ
    // ビルドは通らない。両方の存在を固定する。
    let dockerfile = read_dockerfile();
    assert!(
        dockerfile.contains("FROM distro-${VIBEPOD_PROFILE} AS base"),
        "expected the distro selector `FROM distro-${{VIBEPOD_PROFILE}} AS base` in \
         templates/Dockerfile"
    );
    assert!(
        dockerfile.contains("FROM profile-${VIBEPOD_PROFILE}"),
        "expected the trailing profile selector `FROM profile-${{VIBEPOD_PROFILE}}` in \
         templates/Dockerfile"
    );
}

#[test]
fn swift_version_defaults_to_a_pinned_release() {
    let dockerfile = read_dockerfile();
    let block = provisioning_block(&dockerfile);
    assert!(
        block.contains("ARG SWIFT_VERSION=6.3.3"),
        "expected `ARG SWIFT_VERSION=6.3.3` (a pinned default) in templates/Dockerfile's \
         provisioning block so a plain `docker build` reproducibly installs a known Swift \
         toolchain version"
    );
}

#[test]
fn swift_checksum_table_contains_both_verified_arch_hashes() {
    let dockerfile = read_dockerfile();
    let block = provisioning_block(&dockerfile);
    assert!(
        block.contains(SWIFT_AARCH64_SHA256),
        "expected the verified aarch64 SHA256 ({SWIFT_AARCH64_SHA256}) for Swift 6.3.3 to be \
         present in templates/Dockerfile's provisioning block"
    );
    assert!(
        block.contains(SWIFT_X86_64_SHA256),
        "expected the verified x86_64 SHA256 ({SWIFT_X86_64_SHA256}) for Swift 6.3.3 to be \
         present in templates/Dockerfile's provisioning block"
    );
}

#[test]
fn swiftlint_version_defaults_to_a_pinned_release() {
    let dockerfile = read_dockerfile();
    let block = provisioning_block(&dockerfile);
    assert!(
        block.contains("ARG SWIFTLINT_VERSION=0.65.0"),
        "expected `ARG SWIFTLINT_VERSION=0.65.0` (a pinned default) in templates/Dockerfile's \
         provisioning block"
    );
}

#[test]
fn swiftlint_checksum_table_contains_both_verified_arch_hashes() {
    let dockerfile = read_dockerfile();
    let block = provisioning_block(&dockerfile);
    assert!(
        block.contains(SWIFTLINT_ARM64_SHA256),
        "expected the verified arm64 SHA256 ({SWIFTLINT_ARM64_SHA256}) for SwiftLint 0.65.0 to \
         be present in templates/Dockerfile's provisioning block"
    );
    assert!(
        block.contains(SWIFTLINT_AMD64_SHA256),
        "expected the verified amd64 SHA256 ({SWIFTLINT_AMD64_SHA256}) for SwiftLint 0.65.0 to \
         be present in templates/Dockerfile's provisioning block"
    );
}

#[test]
fn swift_path_is_exported_for_login_shells_via_profile_d() {
    // 実欠陥（E2E スモークで発覚）: エージェントは `bash --login` で起動
    // されるが、Debian の /etc/profile が PATH をリセットするため、
    // `ENV PATH=/opt/swift/usr/bin:$PATH` だけでは login シェル内で
    // `swift` が PATH に無い（swiftlint は /usr/local/bin にあるため影響
    // を受けない）。/etc/profile は PATH 設定後に /etc/profile.d/*.sh を
    // source するため、ここに書けば login シェルでも有効になる。
    let dockerfile = read_dockerfile();
    let block = provisioning_block(&dockerfile);
    assert!(
        block.contains("/etc/profile.d/swift-toolchain.sh")
            && block.contains("export PATH=/opt/swift/usr/bin:$PATH"),
        "expected a PATH export written to /etc/profile.d/swift-toolchain.sh so login shells \
         (`bash --login`, which is how the agent is started) pick up the Swift toolchain: {block}"
    );
    assert!(
        // Copilot 指摘（PR #61）: /etc/profile 側の source 判定は `[ -r $i ]`
        // （可読かどうか）であり、ビルド時の umask 次第で非可読（例: 0600、
        // root のみ）になると無言でスキップされ、vibepod ユーザーの login
        // シェルで再び swift が PATH から消える。umask に頼らず 0644 を
        // 明示していることを固定する。
        block.contains("chmod 0644 /etc/profile.d/swift-toolchain.sh"),
        "expected the profile.d script's permissions to be explicitly set to 0644 (not left to \
         the build-time umask), since /etc/profile silently skips unreadable scripts: {block}"
    );
}

#[test]
fn smoke_check_runs_through_a_login_shell() {
    // 実欠陥回帰: 旧スモーク `RUN swift --version && swiftlint version` は
    // 非 login シェルで実行されるため、`ENV PATH` だけで通ってしまい、
    // login シェルで `swift` が見つからないこの欠陥クラスを構造的に検知
    // できなかった。`bash --login -c '...'` 経由にすることで、エージェント
    // の実際の起動方法（`bash --login`）と同じ経路をビルド時に通す。
    let dockerfile = read_dockerfile();
    assert!(
        dockerfile.contains("RUN bash --login -c 'swift --version && swiftlint version'"),
        "expected the smoke check to run through a login shell (`bash --login -c '...'`), not a \
         plain `RUN swift --version && swiftlint version`, so PATH regressions that only affect \
         login shells are caught at build time instead of only in E2E: {dockerfile}"
    );
}

#[test]
fn swift_and_swiftlint_provisioning_has_no_latest_escape_hatch() {
    let dockerfile = read_dockerfile();
    let block = provisioning_block(&dockerfile);

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
