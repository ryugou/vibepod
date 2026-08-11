# Changelog

All notable changes to VibePod are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.9.0] - 2026-08-11

### Changed

- **Behavior change:** `vibepod init` no longer removes existing containers without asking. Previously it deleted every VibePod container across every project without confirmation whenever no container happened to be running. Stopped containers hold resumable session state, so that path silently destroyed other projects' work. From this release, if even one VibePod container exists — running or stopped, no state is treated as "safe to delete" — an interactive terminal gets a confirmation prompt and a non-interactive one aborts without touching anything. Only a completely empty container list proceeds unprompted. **This will make `vibepod init` fail in CI or scripts whenever any VibePod container exists**; run it from an interactive terminal, or remove the containers deliberately first. The abort message names `vibepod ps` so you can see what exists before deciding
- **Behavior change:** container state is now read from Docker's machine-readable `{{.State}}` instead of the human-facing `{{.Status}}` string. The old heuristic (`starts_with("Up") || contains("running")`) failed to recognize `Restarting (...)`, so a container that was effectively live could be force-removed. Only `exited`, `created`, and `dead` are treated as safe to remove; everything else — including states Docker may add in the future — is protected
- **Behavior change:** `vibepod login` now requires an interactive terminal on both stdin and stderr, and checks this before doing any work. It previously only guarded the "overwrite existing token?" prompt, so a run without an existing token reached `docker exec -it` and died on Docker's opaque `the input device is not a TTY`. stdin is genuinely required by the OAuth flow; stderr is only strictly needed for the overwrite prompt, but the check does not branch on token presence — keeping the command's precondition simple is a deliberate product decision, and the error message says so rather than claiming a technical necessity that isn't there
- **BREAKING:** `DockerRuntime::list_vibepod_containers` now returns `Vec<ContainerInfo>` (a struct with `name`, `state`, and `status`) instead of `Vec<(String, String)>`. `runtime` is re-exported from the crate root, so this affects anyone using VibePod as a library. The extra field is what makes state-based protection possible

### Fixed

- `vibepod init`, `vibepod run`, and `vibepod restore` crashed with `IO error: not a terminal` in non-interactive environments. dialoguer prompts on `Term::stderr()`, and none of these commands checked for a terminal before reaching one. Each is now handled according to what the prompt actually decides: `init` falls back to the default agent (announced on stderr), `run` auto-registers an unregistered project (matching what `--prompt` / `--prompt-file` / `--resume` already did), and `restore` refuses up front — picking a session and confirming a destructive restore are not decisions to make on the user's behalf
- `vibepod stop --all` skipped containers in the `restarting` state, reporting no running containers while leaving live ones untouched. It now selects by `state` (`running` and `restarting`); `paused` is deliberately excluded, since silently unpausing what a user explicitly paused is not what `stop` should do
- A malformed line from `docker ps` was silently discarded. If every line failed to parse, callers saw an empty list — indistinguishable from "no containers" and, in the removal path, the one value that means "proceed without asking." Parse failures are now propagated as errors; the message reports field counts only, never the line contents or a container name

### Added

- Integration tests covering the container-removal path, not just the pure decision functions. A minimal `ContainerRegistry` trait (list and remove only) allows a fake to record whether removal was actually called. The tests pin that removal never happens on a non-interactive abort, a declined confirmation, an empty list, or a listing failure; that a container appearing mid-build still aborts on the pre-removal recheck; and that a mid-sequence removal failure stops the loop. `vibepod init` and the tests now share one orchestration function, so dropping the pre-removal recheck from the production path is a test failure rather than a silent regression

## [1.8.2] - 2026-08-10

### Added
- The container image now bundles Node.js (v22.23.2 LTS, pinned with per-architecture SHA256 verification before extraction, no `latest` escape hatch), so a `codex` review can run to completion inside the container. The `codex` CLI itself is a musl static binary that needs no runtime, but the Codex Claude Code plugin reaches it through `codex-companion.mjs`, which requires node — without it the in-container review flow stopped at that step every time. Node's C++ addon headers are removed in the same layer they are unpacked in (they serve no purpose here, and dropping them keeps 59MB out of the image); `npm` is kept. Net image growth is roughly 135MB

## [1.8.1] - 2026-08-10

### Added
- `vibepod run` prints the resolved profile and image at startup — `Profile: swift (image: vibepod-claude-swift:latest)`, or `Profile: default (image: vibepod-claude:latest)` when no profile is set — in both the `--prompt` and interactive output formats. `profile` is a configuration-file-only setting with no CLI flag, and there was previously no way to confirm it had taken effect short of inspecting containers with `docker ps -a`. The line is always printed, so a setting that failed to apply is visible in the same place and format as one that did
- Session metadata (`.vibepod/sessions/<id>/metadata.json`) records the `image` and `profile` used for the run, making it possible to determine after the fact which image a past session actually ran on. Metadata files written by earlier versions still load, with both fields defaulting to `null`
- The `--lang` help text and the README option table point at `profile = "swift"`. `--lang` accepts no `swift` value, so reading the help alone suggested Swift was unsupported

### Changed
- `--prompt` / `--prompt-file` runs prepend a short environment block to the prompt handed to the in-container agent, stating which toolchains are actually present. With `profile = "swift"` it reports that Swift and SwiftLint are installed and must not be reinstalled; with no profile set but a `Package.swift` in the workspace it reports that they are absent, that installing them will fail on missing shared libraries, and that Swift build/test/lint should be reported as not run — other languages are explicitly outside that restriction, so a polyglot repository still gets its other toolchains verified. The prepended text reaches the agent only: the session lock key, `Session.prompt`, and `--verbose` log output all keep the original prompt unchanged

## [1.8.0] - 2026-08-05

### Added
- Swift language profile: setting `profile = "swift"` under `[run]` in `.vibepod/config.toml` selects an image variant with the Swift toolchain and SwiftLint baked in — Swift 6.3.3 and SwiftLint 0.65.0, both pinned with per-architecture SHA256 verification before extraction and no `latest` escape hatch. Only the `swift` profile's image is built on Debian 13 (trixie) instead of Debian 12 (bookworm); this is required because SwiftLint's official Linux binary needs a newer glibc than bookworm ships, while the default image is unaffected and stays on bookworm. The Swift toolchain's `PATH` is set both via `ENV` and a `/etc/profile.d` script (with permissions explicitly set to `0644`) so it's available in both non-login shells and the login shell the agent actually starts (Debian's `/etc/profile` resets `PATH` for login shells, so `ENV` alone isn't enough). The swift-profile image (`vibepod-claude-swift`, derived from the configured image name) is auto-built on first use just like the default image; `vibepod init --rebuild` now rebuilds every profile variant that has previously been built, not just the default. Projects with a `Package.swift` but no `profile` set get a one-line hint on each run pointing at this setting (the run continues normally). Constraints: only Foundation-only, pure SwiftPM packages build and run on Linux — no Apple frameworks, `xcodebuild`, simulators, `lldb`, or `swift repl`
- `--prompt-file <path>`: reads the `--prompt` text verbatim from a file instead of a shell argument, avoiding host-shell interpretation of special characters (`<`, `{`, backticks, `$`, ...); mutually exclusive with `--prompt`
- Timeout recovery guidance now reflects the actual workspace state: uncommitted changes, committed-only and clean, unchanged since the session started, or undeterminable because a git status probe failed, each combined with whether `--worktree` was used. Only commands that will actually succeed in that state are suggested (`vibepod restore`, a discard command, or `--worktree`-scoped `git -C` commands) — a probe failure is reported as "state unknown" rather than guessed at, and no destructive or bound-to-fail command is ever offered

### Changed
- **Behavior change:** a `--prompt` session that hits its timeout no longer resets the workspace (1.7.x auto-reset it — a hard reset to the starting commit when the tree was clean at session start, or a mixed reset of HEAD when it wasn't). The agent's changes — commits and uncommitted edits alike — are now always preserved; revert them with `vibepod restore` or the manual command shown in the timeout guidance
- **Behavior change:** a malformed `.vibepod/config.toml` or global `config.toml` (TOML syntax error, a type error such as `profile = 123`, or a read error such as a permissions issue) now makes `vibepod run` fail explicitly instead of silently treating it as "no config". A genuinely missing config file is still treated as no config, as before

### Fixed
- `--timeout` values that aren't a whole number of minutes are no longer truncated in the timeout message (`--timeout 90` used to display as 1 minute, dropping the 30 seconds)

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
