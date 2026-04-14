# Changelog

All notable changes to VibePod are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.6.0] - 2026-04-??

### Added
- `--lang <rust|go|node|python|java>` now selects an official bundle (agents, skills, and toolchain) and becomes the primary entry for language-specific autonomous runs
- `--mode impl|review` flag on `vibepod run`, default `impl`. `--mode review` mounts a reviewer-focused read-only bundle with modification commands blocked via `permissions.deny`
- `vibepod template update [--ref <ref>]` to refresh the local ecc cache manually (blocking fetch)
- `vibepod template status` to show ecc cache state (repo, ref, last fetch time, current commit)
- `[ecc]` section in `vibepod-template.toml` lists skill/agent paths to pull from the ecc cache; path-safety validated at parse time (no absolute paths, no `..` traversal, no empty entries, required `skills/` / `agents/` prefixes)
- Auto-refresh of the ecc cache via background `git fetch` (TTL-based, configurable via `[ecc]` in `config.toml`)
- Language bundles: `rust/impl`, `rust/review`, `go/impl`, `go/review`, `node/impl`, `node/review`, `python/impl`, `python/review`, `java/impl`, `java/review`, plus language-agnostic `generic/review`
- Custom templates can opt into ecc content by adding an `[ecc]` section to their `vibepod-template.toml`

### Changed
- `vibepod init` now clones the ecc repository into `~/.config/vibepod/ecc-cache/` (or `git fetch` if it already exists — idempotent)
- `--template` is now for custom templates only. Combining `--template <name>` with `--mode review` is rejected at CLI parse-time
- `vibepod template status` surfaces git errors explicitly instead of printing `unknown`

### Removed
- Bundled `templates-data/rust-code/`, `templates-data/review/`, `templates-data/rust-code-codex/` — agent/skill content is now sourced from the ecc cache per bundle
- 8 tests specific to the flat legacy bundle layout (replaced with v1.6-nested-aware regression gates for idempotence, sibling-conflict isolation, rust-analyzer setup declaration)

### Security
- `review` bundles (per-language and generic) block modification-side shell commands via layered `permissions.deny` — git mutators, filesystem mutators, language-specific package manager mutators, and runtime-specific dangerous-code-execution commands (e.g. `jshell`, `npx`, `pnpm dlx`)
- Staging-dir assembly rejects symbolic links in custom template source trees to preserve v1.5's template-escape protection
- Per-language review bundles include `santa-method` dual-reviewer convergence triggers keyed to the language's highest-risk primitives (Rust `unsafe`, Go `cgo`/`unsafe`, Node `eval`/prototype pollution, Python `pickle`/`eval`, Java JNDI/reflection/XXE)
