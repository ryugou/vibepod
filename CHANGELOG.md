# Changelog

All notable changes to VibePod are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.7.1] - 2026-07-22

### Added
- The container image now bundles the `codex` CLI (musl static binary, no node/npm required) so in-container `codex` review can run; the version is pinned via the `CODEX_VERSION` build arg (default `0.145.0`) and its SHA256 checksum is verified before extraction, failing the build on a mismatch or an unrecognized version/arch pin. `--build-arg CODEX_VERSION=latest` remains as an explicit escape hatch that skips checksum verification
- Only the host's `~/.codex/auth.json` and `config.toml` (never `history.jsonl`, `goals_*.sqlite`, or `cache/`) are injected into the container. A per-container rw-mounted "shown" stage (disposed with the runtime dir on cleanup) is kept fully separate from a host-only "persisted" auth store that is never mounted into any container; syncing between host, store, and stage keeps the newer `auth.json` (preserving in-container token refresh), reconciles and rejects symlinked or non-regular entries, enforces `0600` permissions on staged files, serializes concurrent `vibepod run` invocations with a flock, and gates the post-run write-back on both a full JSON parse of the refreshed `auth.json` and a confirmed docker command exit status

### Security
- Hardened the container-to-host boundary for the injected codex credentials: the shown stage and the persisted auth store are fully separated, symlinked or non-regular files are rejected both on host read and stage write, staged files are forced to `0600`, concurrent runs are serialized via flock, and the post-run sync back to the store is gated on both JSON-validity of the refreshed auth data and the triggering docker command's exit status — closing TOCTOU paths a tampered container could otherwise use to corrupt or exfiltrate host-side auth

## [1.7.0] - 2026-07-21

### Added
- Host `~/.claude/` (`CLAUDE.md`, `agents/`, `skills/`, `specs/`, `plugins/`) is now mounted read-only into every `vibepod run` container via an allowlist, regardless of mode; `--lang`/`--template` runs previously lost the user's CLAUDE.md, agents, and skills, and `~/.claude/specs/` was never mounted at all
- Claude Code inside the container self-updates automatically, throttled to once per 24 hours; `--update` forces an immediate check and `--no-update` disables checking entirely
- `vibepod init --rebuild` forces a clean image rebuild (`docker build --pull --no-cache`) to pick up a fresh `install.sh` layer
- `vibepod run` auto-builds the Docker image on demand when it is missing (a few minutes), instead of erroring; concurrent runs are serialized by a build lock. Use `--no-auto-build` to opt out and fail fast instead
- `--model <name>` passes `--model <name>` straight through to Claude Code inside the container (validated by Claude Code itself, not VibePod)
- `--timeout <dur>` sets a wall-clock limit for `--prompt` sessions (seconds or `30m`/`1h30m` duration syntax, `0` disables it), defaulting to 30 minutes; on timeout the container-side agent is stopped and, when the workspace was clean at session start, it is reset to the starting commit (pre-existing uncommitted changes are preserved, not discarded)
- `--prompt` sessions print a concise end-of-run summary (files changed, duration, exit status) by default; `--verbose` restores the pre-1.7 behavior of streaming Claude Code's per-event activity to stdout

### Changed
- Non-interactive runs (`--prompt` / `--resume`) now always pass `--dangerously-skip-permissions` to Claude Code; the sandbox boundary is the Docker container itself, not Claude Code's own permission prompts

### Removed
- **BREAKING:** the template mechanism — `--template`, the `vibepod template` subcommand (`status`/`update`), the ecc cache, and all bundled `templates-data/` — has been dropped. `~/.config/vibepod/templates/` is no longer read by any command
- **BREAKING:** `--mode impl|review` on `vibepod run` has been dropped along with the review-mode bundle

## [1.6.1] - 2026-04-18

### Fixed
- `vibepod run --prompt` failed with exit 1 on the second invocation when reusing a running container. Root cause: `has_claude_process` used `docker top -o cmd`, which Docker Desktop's ps backend rejects as `bad -o argument 'cmd'`. Switched the probe to `docker top -o pid,args` (both columns portable) and added a guard so the probe is skipped for non-running containers (#51)

## [1.6.0] - 2026-04-11

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
