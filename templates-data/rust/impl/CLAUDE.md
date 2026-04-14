# Rust Implementation (vibepod)

## Rules (universal + Rust-specific)

- Write tests before implementation. Follow `skill: tdd-workflow`.
- `unwrap()` and `expect()` are forbidden in production code paths. In test code (`#[cfg(test)]` modules and `tests/` integration tests) they are allowed. For deliberate use in production (e.g. a regex compiled from a literal pattern that cannot fail), add a comment explaining why the call cannot panic.
- Propagate errors with `?`. Add context at the call site when the cause is not self-evident (via `anyhow::Context::with_context(...)` for applications, or a typed error variant for libraries — `skill: rust-patterns` covers both).
- Do not swallow errors (`let _ = result;` on `Result<_, _>`) without a comment explaining why.
- No stray `dbg!`, `println!`, or `eprintln!` in committed code. Use the `tracing` or `log` crate for diagnostics.
- Do not commit `panic!()`, `todo!()`, or `unreachable!()` on reachable paths.
- No hardcoded secrets.
- Before declaring work complete: `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` must all pass.

## Agents (prefer in this order)

- `rust-reviewer` — Rust-specific review (ownership, unsafe, error handling, idioms).
- `rust-build-resolver` — when `cargo build` or `cargo check` fails.
- `code-architect` — design / planning before implementation.
- `code-explorer` — tracing existing code paths in an unfamiliar crate.
- `silent-failure-hunter` — before commit, sweep for swallowed errors and bad fallbacks.
- `code-reviewer` — generic fallback for cross-cutting concerns (API shape, naming, comments) when the rust-specific reviewer has nothing to add.

## Skills (load as needed)

- `rust-patterns` — idiomatic ownership, error handling with `thiserror`/`anyhow`, trait design.
- `rust-testing` — `#[test]`, `rstest`, `proptest`, `mockall`, `cargo-llvm-cov` coverage.
- `tdd-workflow` — strict RED → GREEN → REFACTOR discipline with git checkpoints.
