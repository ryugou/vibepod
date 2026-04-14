# Rust Implementation (vibepod)

## Rules (universal + Rust-specific)

- Write tests before implementation. Follow `skill: tdd-workflow`.
- `unwrap()` / `expect()` is forbidden in production code paths. In `#[cfg(test)]` code they are allowed. For deliberate use (e.g. a regex compiled from a literal pattern that cannot fail), add a comment explaining why the call cannot panic.
- Propagate errors with `?`. Add context via `anyhow::Context::with_context(...)` when the call site is not self-evident.
- Do not swallow errors (`let _ = result;` on `Result<_, _>`) without a comment explaining why.
- Do not commit `panic!()`, `todo!()`, or `unreachable!()` on reachable paths.
- No hardcoded secrets.
- Before declaring work complete: `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` must all pass.

## Agents (prefer in this order)

- `rust-reviewer` — Rust-specific review (ownership, unsafe, error handling, idioms).
- `rust-build-resolver` — when `cargo build` or `cargo check` fails.
- `code-architect` — design / planning before implementation.
- `code-explorer` — tracing existing code paths in an unfamiliar crate.
- `silent-failure-hunter` — before commit, sweep for swallowed errors and bad fallbacks.

## Skills (load as needed)

- `rust-patterns` — idiomatic ownership, error handling with `thiserror`/`anyhow`, trait design.
- `rust-testing` — `#[test]`, `rstest`, `proptest`, `mockall`, `cargo-llvm-cov` coverage.
- `tdd-workflow` — strict RED → GREEN → REFACTOR discipline with git checkpoints.
