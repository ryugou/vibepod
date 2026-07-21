//! Static regression guard for `templates/Dockerfile`'s `codex` CLI
//! provisioning layer (Copilot review round C2, 2026-07-22).
//!
//! Background: the layer used to fetch `codex` from
//! `releases/latest/download/<asset>` with no version pin and no checksum
//! verification — a supply-chain / reproducibility gap (a compromised or
//! yanked "latest" release would be installed silently, and rebuilding the
//! image at different times could pull different binaries). The fix pins a
//! default `CODEX_VERSION` build arg, verifies the downloaded asset's SHA256
//! against a checksum table before installing it, and only allows skipping
//! verification via an explicit `--build-arg CODEX_VERSION=latest` escape
//! hatch that also emits a build-log warning.
//!
//! This environment has no `docker` binary, so an actual `docker build`
//! (pinned success, and a tampered-checksum failure) cannot be exercised
//! here. These tests instead assert on the *structure* of the Dockerfile
//! text so that removing the pin, the checksum verification, or the
//! unknown-version failure path regresses a test even without a real build.
//! They intentionally avoid pinning to the exact shell formatting so minor
//! rewording/reflow of the `RUN` block doesn't make this test brittle.

use std::fs;
use std::path::PathBuf;

/// SHA256 for `codex-aarch64-unknown-linux-musl.tar.gz` at `rust-v0.145.0`,
/// verified out-of-band (round C2) via `gh release view` + a real download.
const AARCH64_SHA256: &str = "d384f90bc842450b42bd675feef06a12a46a3b1ca97efcb22566b270e4a11227";

/// SHA256 for `codex-x86_64-unknown-linux-musl.tar.gz` at `rust-v0.145.0`,
/// verified out-of-band (round C2) via `gh release view` + a real download.
const X86_64_SHA256: &str = "bfaf13c9ba34f2ad764e4a916c49cf7177aeba329cf0f719e2227566fc8d662a";

fn read_dockerfile() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates/Dockerfile");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

#[test]
fn codex_version_defaults_to_a_pinned_release() {
    let dockerfile = read_dockerfile();
    assert!(
        dockerfile.contains("ARG CODEX_VERSION=0.145.0"),
        "expected `ARG CODEX_VERSION=0.145.0` (a pinned default, not `latest`) in \
         templates/Dockerfile, so a plain `docker build` reproducibly installs a known codex \
         version instead of whatever `releases/latest` happens to resolve to at build time"
    );
}

#[test]
fn pinned_download_url_targets_the_versioned_release_tag() {
    let dockerfile = read_dockerfile();
    assert!(
        dockerfile.contains("releases/download/rust-v${CODEX_VERSION}/"),
        "expected the non-latest download URL to be built from \
         `releases/download/rust-v${{CODEX_VERSION}}/...` so a pinned CODEX_VERSION fetches an \
         immutable, versioned release asset rather than the mutable `releases/latest` alias"
    );
}

#[test]
fn checksum_table_contains_both_verified_arch_hashes() {
    let dockerfile = read_dockerfile();
    assert!(
        dockerfile.contains(AARCH64_SHA256),
        "expected the verified aarch64 SHA256 ({AARCH64_SHA256}) for codex rust-v0.145.0 to be \
         present in templates/Dockerfile's checksum table"
    );
    assert!(
        dockerfile.contains(X86_64_SHA256),
        "expected the verified x86_64 SHA256 ({X86_64_SHA256}) for codex rust-v0.145.0 to be \
         present in templates/Dockerfile's checksum table"
    );
}

#[test]
fn downloaded_asset_is_verified_with_sha256sum_before_install() {
    let dockerfile = read_dockerfile();
    assert!(
        dockerfile.contains("sha256sum -c"),
        "expected the codex asset download to be checked with `sha256sum -c` (or equivalent \
         `-c -` piped form) before the archive is extracted and installed, so a tampered or \
         mismatched download fails the build instead of being silently installed"
    );
    // The checksum check must run before extraction, not after: an
    // after-the-fact check would leave a corrupted/tampered binary already
    // unpacked (and potentially installed) by the time verification fails.
    let sha_pos = dockerfile
        .find("sha256sum -c")
        .expect("sha256sum -c must be present (checked above)");
    let tar_pos = dockerfile
        .find("tar -xzf")
        .expect("expected a `tar -xzf` extraction step for the downloaded codex archive");
    assert!(
        sha_pos < tar_pos,
        "expected the sha256sum verification to run before `tar -xzf` extracts the downloaded \
         archive, so a checksum mismatch aborts before any untrusted content is unpacked"
    );
}

#[test]
fn codex_version_latest_takes_an_unverified_escape_hatch_with_a_warning() {
    let dockerfile = read_dockerfile();
    assert!(
        dockerfile.contains("releases/latest/download/"),
        "expected a `releases/latest/download/...` branch to remain as the explicit \
         `CODEX_VERSION=latest` escape hatch"
    );
    assert!(
        dockerfile.to_lowercase().contains("codex_version") && dockerfile.contains("latest"),
        "expected a conditional branch keyed on CODEX_VERSION being \"latest\""
    );
    assert!(
        dockerfile.contains("skipping checksum verification"),
        "expected a build-log warning (stderr echo) stating that checksum verification is \
         skipped when CODEX_VERSION=latest, so the reduced-safety escape hatch is not silent"
    );
}

#[test]
fn unknown_pinned_version_or_arch_fails_the_build_instead_of_skipping_verification() {
    let dockerfile = read_dockerfile();
    assert!(
        dockerfile.contains("no known SHA256 for CODEX_VERSION"),
        "expected an explicit failure message for a CODEX_VERSION/arch combination absent from \
         the checksum table, so a pin bump that forgets to add checksums fails loudly at build \
         time instead of silently installing an unverified binary"
    );
    assert!(
        dockerfile.contains("exit 1"),
        "expected the unknown-checksum fallback (and the unsupported-architecture fallback) to \
         `exit 1`, actually failing the build rather than just logging a warning and continuing"
    );
}

#[test]
fn codex_version_still_verified_with_codex_version_flag() {
    // Regression guard: the final sanity check that the installed codex
    // binary actually runs must survive this change untouched.
    let dockerfile = read_dockerfile();
    assert!(
        dockerfile.contains("RUN codex --version"),
        "expected the existing `RUN codex --version` smoke check to remain after the pin / \
         checksum changes"
    );
}
