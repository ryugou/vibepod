# VibePod

Safely run AI coding agents in Docker containers.

VibePod wraps Docker to let you run [Claude Code](https://docs.anthropic.com/en/docs/claude-code) inside an isolated container — set up in three steps.

## Quick Start

```bash
# Install (see below for other methods)
brew tap ryugou/tap
brew install vibepod

# Build the Docker image (one-time setup)
vibepod init

# Authenticate for container use (one-time)
vibepod login

# Run interactively inside a safe container
cd your-project
vibepod run

# Or fire-and-forget with a prompt
vibepod run --prompt "Implement the login page"
```

## Commands

### `vibepod init`

Builds the Docker image and creates global configuration. Detects your host UID/GID automatically for seamless file permissions.

| Option | Description |
|--------|-------------|
| `--rebuild` | Rebuild from scratch with `docker build --pull --no-cache`. The image's Claude Code install step is cached on its command text, which never changes, so a plain `vibepod init` re-run replays the cached layer and reinstalls the *same* version. Use this when you want a genuinely fresh image |

Day to day you should not need `--rebuild`: `vibepod run` keeps the container's Claude Code current on its own (see below).

#### Keeping Claude Code up to date

The image pins whatever Claude Code version existed when you last built it. To stop containers drifting, `vibepod run` runs `claude update` inside the container before starting the session:

- **Throttled** to once per 24h per container; a freshly created container always checks, since its binary comes straight from the image.
- **Never fatal.** A failed check warns — with the cause, the manual command, and the opt-out — and the session continues on the installed version.
- Skipped automatically under `--no-network`.
- Timestamps live in `~/.config/vibepod/update-check.json`. Force with `--update`, disable with `--no-update`.

### `vibepod login`

Authenticates for container use. Creates a dedicated OAuth session stored in `~/.config/vibepod/auth/token.json`. This session is separate from your host's Claude credentials and is used when running containers.

```bash
vibepod login
```

### `vibepod logout`

Removes the shared authentication session.

```bash
vibepod logout
```

### `vibepod restore`

Restores the workspace to a previous session's state. VibePod automatically records the git HEAD at the start of each `vibepod run` session. If the agent makes unwanted changes, you can revert them with a single command.

```bash
vibepod restore
```

This will:
1. Show a list of restorable sessions
2. Generate a Markdown report of all changes (saved to `.vibepod/reports/`)
3. Run `git reset --hard` + `git clean -fd` to restore the workspace

### `vibepod ps`

Lists VibePod containers (running and stopped).

```bash
vibepod ps
```

### `vibepod stop`

Stop VibePod containers (without removing them). Stopped containers are reused on next `vibepod run`.

```bash
vibepod stop <name>
vibepod stop --all
```

### `vibepod rm`

Remove VibePod containers.

```bash
vibepod rm <name>
vibepod rm --all
```

| Argument | Description |
|----------|-------------|
| `<name>` | Name of the container to remove |
| `--all` | Remove all VibePod containers |

### `vibepod logs`

Shows logs from a VibePod container.

```bash
vibepod logs
vibepod logs --tail 50
```

### `vibepod run`

Runs an AI coding agent inside a container, mounting your project directory.

| Option | Description |
|--------|-------------|
| *(none)* | **Interactive mode** — opens a Claude Code session inside the container |
| `--prompt "..."` | Fire-and-forget mode — agent runs autonomously and exits when done |
| `--prompt-file <path>` | Same as `--prompt`, but reads the prompt from a file. The content is passed through unmodified, bypassing host shell interpretation of special characters (`<`, `{`, backticks, `$`, ...). Mutually exclusive with `--prompt` |
| `--resume` | Continue from the previous session (fire-and-forget) |
| `--no-network` | Disable container networking |
| `--env KEY=VALUE` | Pass environment variables (repeatable) |
| `--env-file <path>` | Load environment variables from file (`op://` references resolved via 1Password CLI) |
| `--lang <name>` | Install a language toolchain in the container (`rust`, `node`, `python`, `go`, `java`). Auto-detected from project files if omitted |
| `--worktree` | Run in an isolated git worktree (requires `--prompt`). Changes are made in `.worktrees/` instead of your working tree |
| `--mount <src:dst>` | Mount additional host path into the container (read-only, repeatable) |
| `--new` | Recreate the container from scratch. Removes a stopped container automatically; if the container is running, stop it first with `vibepod stop` or `vibepod rm` |
| `--update` | Check for a Claude Code update inside the container now, ignoring the once-a-day throttle |
| `--no-update` | Skip the container's Claude Code update check entirely |
| `--model <name>` | Pass `--model <name>` straight through to Claude Code inside the container. Not validated by VibePod — Claude Code decides if it is valid. Works in both interactive and `--prompt` mode. Omit to use Claude Code's own default |
| `--no-auto-build` | Do not build the Docker image on demand when it is missing. By default `vibepod run` auto-builds it; pass this to fail fast and be told to run `vibepod init` instead |
| `--timeout <dur>` | Wall-clock limit for a `--prompt` session. Accepts bare seconds (`1800`) or a duration (`30m`, `1h30m`); `0` disables it. Defaults to **30 minutes**. On timeout the container-side agent is stopped and the run exits non-zero; workspace changes are left in place, not reset. Recovery depends on the state found: `vibepod restore` only works with a clean tree — uncommitted changes must be committed or discarded (`git reset --hard && git clean -fd`, irreversible) first; `--worktree` runs point you at `.worktrees/<dir>` instead (see below) |
| `--verbose` | Stream Claude Code's per-event activity to stdout during `--prompt` (pre-1.7 behavior). By default only a concise end-of-run summary is printed |

**Image auto-build.** The first `vibepod run` in an environment where the image is missing builds it automatically (a few minutes) instead of erroring, so you can call `vibepod run` from another session without running `vibepod init` first. Concurrent runs are serialized by a build lock so the image is built once. Use `--no-auto-build` to opt out.

**Container reuse is the default.** VibePod creates one container per project (named `vibepod-{project}-{hash}`) and reuses it across runs. Setup only runs once; subsequent `vibepod run` calls skip setup and connect instantly via `docker exec`. Use `--new` to force a fresh container.

#### Your host `~/.claude/` always comes along

VibePod carries these host assets into the container (read-only, skipped when absent):

- `~/.claude/CLAUDE.md`
- `~/.claude/agents/`
- `~/.claude/skills/`
- `~/.claude/specs/`
- `~/.claude/plugins/`

This is an **allowlist**. Session and history data — `sessions/`, `projects/`, `history.jsonl`, `backups/`, `file-history/`, `shell-snapshots/`, `todos/` — is never copied into the container, both because it is large and because it contains other projects' conversations.

`settings.json` is mounted as a sanitized copy of yours (`hooks` and `statusLine` stripped), written to `~/.config/vibepod/runtime/<container>/settings.json`.

Because bind mounts are fixed at container creation, adding a new asset to `~/.claude/` (a first-ever `specs/`, say) changes the mount set and requires `vibepod run --new` to take effect. VibePod detects this and tells you.

#### When to use which?

- **`vibepod run`** (interactive) — day-to-day development. You get a normal Claude Code session safely inside a Docker container. Permission prompts work normally — no bypass mode. The container persists for instant reconnection.
- **`--prompt`** (fire-and-forget) — when the spec is already written and you want to kick off autonomous execution with `--dangerously-skip-permissions`. Great for running overnight or during meetings. Pair with a spec file in your repo: `vibepod run --prompt "Follow specs/login.md and implement"`.
- **`--prompt --worktree`** — same as above, but runs in an isolated git worktree. Your working tree stays untouched. Review the changes before merging. Always creates a fresh container.

#### Passing secrets with 1Password

Create a `.env.template` with `op://` references (safe to commit to Git):

```
GITHUB_TOKEN="op://ai-agents/GitHub/token"
DB_URL="op://ai-agents/PostgreSQL/url"
```

VibePod resolves them via 1Password CLI before passing to the container:

```bash
vibepod run --env-file .env.template
```

#### Codex review inside the container

The container image bundles the `codex` CLI (musl static binary, no node/npm
required) so the implementation-delegation flow can run a `codex` review
before code leaves the container.

Prerequisites:

- You are logged into codex on the host (`~/.codex/auth.json` exists).

VibePod carries in exactly two files from your host `~/.codex/` — nothing
else:

- `~/.codex/auth.json`
- `~/.codex/config.toml` (if present)

This is an **allowlist**, same policy as `~/.claude/`: `history.jsonl`,
`goals_*.sqlite`, and `cache/` are never copied in, both because they are
unnecessary for running `codex` and because they may contain sensitive data.

The area the container can see and the area that persists on your host are
structurally separate:

- **What the container sees**: a **per-container stage**
  (`~/.config/vibepod/runtime/<container>/codex/`), copied (not bind-mounted
  read-only) and mounted **read-write** at `/home/vibepod/.codex` — the same
  copy-then-mount pattern used for `~/.claude.json`. It's read-write because
  `codex` rewrites `auth.json` on token refresh. Anything `codex` writes
  under `/home/vibepod/.codex` at runtime (session history, sqlite goal
  databases, cache) stays confined to this one container's stage and is
  invisible to every other container. A disposable run (`--new` / worktree)
  deletes its stage along with the rest of its per-container runtime
  directory on exit — that's fine, because persistence lives elsewhere.
- **What's ever written back to your host**: a **host-only auth store**
  (`~/.config/vibepod/codex-auth/`) holding just the same two allowlisted
  files. It is **never mounted into any container.** Before a run starts,
  VibePod syncs host → store → stage: `auth.json` uses keep-newest (so a
  container-refreshed token already sitting in the store isn't clobbered by
  a stale host copy), while `config.toml` always follows the host. After a
  container stops, and before its per-container runtime directory (and thus
  its stage) is deleted, VibePod syncs any refreshed `auth.json` from the
  stage back into the store so the next run can pick it up. Your original
  `~/.codex/auth.json` / `config.toml` are never written to by any of this.

**Accepted risk**: if a container is killed forcibly (`kill -9`, host crash,
etc.) before the post-run sync runs, a token refresh that happened only in
that run's stage is lost — the auth store still holds the last value it saw
from a clean run. Recover by logging into codex again on the host
(`codex login`); the next `vibepod run` picks up the fresh `auth.json`.

If `~/.codex/auth.json` is missing, VibePod prints a note to stderr and
continues without codex support in that container — this is not a fatal
error.

The `codex` binary itself is **not** auto-updated at runtime (unlike Claude
Code). Its version is **pinned** via the `CODEX_VERSION` build arg in
`templates/Dockerfile` (default: a fixed release tag, not `latest`), and the
downloaded release asset is verified against a SHA256 checksum recorded in
that same file — the build fails if the checksum doesn't match. To pick up a
newer `codex` release, bump both `CODEX_VERSION` and the corresponding
SHA256 entries in `templates/Dockerfile`, then rebuild the image with
`vibepod init --rebuild`. As an escape hatch, building the image directly
with `docker build --build-arg CODEX_VERSION=latest -f templates/Dockerfile
-t vibepod-<agent>:latest .` (matching the `vibepod-<agent>:latest` tag
`vibepod init` itself uses) downloads the latest `codex` release from
GitHub without checksum verification. `vibepod init` has no flag for this —
it always builds from the pinned default — so this is a manual, one-off
path (e.g. testing an unreleased pin bump), not part of routine updates.

## Security Model

VibePod provides 3-layer isolation:

1. **Docker container** — the agent runs in an isolated container, not on your host. By default, one container per project is reused across runs; use `--new` or `vibepod rm` to start fresh
2. **Minimal mounts** — only what the agent needs is mounted:
   - `$(pwd)` → `/workspace` (read-write): your project files
   - `~/.claude.json` → container via **temporary copy** (read-write): onboarding state; the host file is never written directly
   - `~/.gitconfig` → `/home/vibepod/.gitconfig` (read-only): git user name and email
   - `~/.claude/CLAUDE.md`, `~/.claude/skills/`, `~/.claude/agents/`, `~/.claude/specs/` (read-only, when present): your personal Claude Code instructions, skills, agents, and specs. This is an **allowlist** — session and history data (`sessions/`, `projects/`, `history.jsonl`, `backups/`, `file-history/`, `shell-snapshots/`, `todos/`) is never mounted
   - `~/.claude/plugins/` (read-only, when present): your installed Claude Code plugins — mounted at both `/home/vibepod/.claude/plugins` and the host absolute path to resolve `installed_plugins.json` entries
   - `~/.claude/settings.json` via **sanitized copy** (read-only, when present): a per-container copy with `hooks` and `statusLine` stripped, written to `~/.config/vibepod/runtime/<container>/settings.json`
   - `~/.codex/auth.json` and `~/.codex/config.toml` (if present, when `auth.json` exists): synced host → host-only auth store (`~/.config/vibepod/codex-auth/`, **never mounted into any container**) → per-container stage (`~/.config/vibepod/runtime/<container>/codex/`, mounted **read-write** at `/home/vibepod/.codex`); read-write because `codex` rewrites `auth.json` on token refresh, which is synced back into the auth store after the container stops and before its per-container runtime directory is deleted
   - `--mount`-specified paths (read-only): additional host paths you explicitly opt in
   - `GH_TOKEN` injected from `gh auth token` when available, for GitHub CLI access inside the container
3. **Git safety net** — your project is git-managed, so any unwanted changes can be reverted with `git reset --hard`

This follows [Anthropic's official recommendation](https://docs.anthropic.com/en/docs/claude-code/security) to use `--dangerously-skip-permissions` only inside containers.

### Interactive vs `--prompt` security model

| Mode | `--dangerously-skip-permissions` | Safety boundary |
|------|----------------------------------|-----------------|
| `vibepod run` (interactive) | **Off** — permission prompts work normally | User approves each action |
| `vibepod run --prompt` | **On** — autonomous execution | Container isolation is the safety boundary |

In interactive mode, Claude Code asks for confirmation before each potentially destructive action. In `--prompt` mode these prompts are bypassed — the container's isolation is what prevents damage to your host.

See [SECURITY.md](SECURITY.md) for the full security details.

## Alias

VibePod can be aliased as `vp` for convenience:

```bash
ln -sf $(which vibepod) /usr/local/bin/vp
vp run --prompt "Fix the failing tests"
```

Note: Homebrew and the install script create this symlink automatically.

## Install

```bash
# macOS (Homebrew)
brew tap ryugou/tap
brew install vibepod

# Linux / macOS (install script)
curl -fsSL https://raw.githubusercontent.com/ryugou/vibepod/main/install.sh | sh

# From source (requires Rust)
cargo install vibepod
```

#### Output in `--prompt` mode

By default, `--prompt` runs print a **concise end-of-run summary** rather than the full `stream-json` activity — the raw stream is verbose and, when `vibepod run` is invoked from another Claude Code session, floods that session's context. The complete stream is always saved to the session `logs.txt` regardless.

```
Summary:
  Status: success
  Result: Implementation complete. All checks pass.
  Changed files (2):
    src/main.rs
    README.md
  Full logs: /path/to/repo/.vibepod/sessions/<id>/logs.txt

Container stopped (container preserved for next run).
```

Pass `--verbose` to stream Claude Code's per-event activity live instead (the pre-1.7 behavior):

```
────────────────────────────────────────────────────────
  │  [assistant] ファイルを確認します。
  │  [tool_use] Read { file_path: "src/main.rs" }
  │  [tool_use] Edit { file_path: "src/main.rs", old_string: "fn main()...", new_string: "fn main()..." }
  │  [tool_use] Bash { command: "cargo check" }
────────────────────────────────────────────────────────
```

If a `--prompt` run exceeds its `--timeout` (default 30 minutes), VibePod stops the container-side agent, prints the `logs.txt` path, and exits non-zero — a timeout is never reported as success. The workspace is **not** reset: any commits and uncommitted edits the agent made are left in place so you can inspect them with `git status` / `git log`. What to do next depends on the state VibePod finds:

- **Uncommitted changes present**: `vibepod restore` refuses to run whenever the tree isn't clean, so it isn't offered here. Discard everything with `git reset --hard && git clean -fd` (irreversible — `git checkout .` alone is not enough, since it leaves staged changes and `git add`-ed new files behind), or keep the changes — leave them as-is, or `git add -A && git commit` them and then run `vibepod restore` if you still want to rewind to the session's start.
- **Only commits, tree clean**: `vibepod restore` works as usual and rewinds to the session's starting commit.
- **`--worktree` runs**: the agent's changes live in `.worktrees/<dir>`, a separate git worktree — `vibepod restore` doesn't apply there (it operates on the current directory's session history, not the worktree). Inspect with `git -C .worktrees/<dir> status` / `git -C .worktrees/<dir> log`, diff against your branch with `git -C .worktrees/<dir> diff main`. Remove the worktree with `git worktree remove .worktrees/<dir>` once you're done with it — if it still has uncommitted changes, that fails and you'll need `git worktree remove --force .worktrees/<dir>` instead (or discard first with `git -C .worktrees/<dir> reset --hard && git -C .worktrees/<dir> clean -fd`).

#### Language toolchain auto-detection

When `--lang` is not specified, VibePod auto-detects the language from project files:

| File | Language |
|------|----------|
| `Cargo.toml` | Rust (+ build-essential) |
| `package.json` | Node.js |
| `go.mod` | Go |
| `pyproject.toml` / `requirements.txt` | Python |
| `pom.xml` / `build.gradle` | Java |

#### Swift profile

Set `profile = "swift"` in the `[run]` section of `.vibepod/config.toml` to use an image variant with the Swift toolchain and SwiftLint pre-installed:

```toml
[run]
profile = "swift"
```

This is configuration-file-only — there is no `--profile` CLI flag. `"swift"` is the only valid value; any other value makes `vibepod run` fail at startup. Like the default image, the swift-profile image is auto-built on the first `vibepod run` that needs it (see "Image auto-build" above).

**Base image.** The swift-profile image is built on Debian 13 (trixie), not Debian 12 (bookworm) like the default image. This is required because SwiftLint's official Linux binary needs a glibc/libstdc++ newer than what bookworm ships (bookworm's `swiftlint version` fails to even start). The Swift toolchain itself still uses the Debian 12 tarball from swift.org (no Debian 13 build is published), which runs on trixie thanks to glibc's backward compatibility (see **Constraints** below for the `lldb` / `swift repl` exception). The default (non-Swift) image is unaffected and stays on Debian 12.

**Version and updates.** The image pins Swift 6.3.3 and SwiftLint 0.65.0. To upgrade, bump the corresponding `ARG` versions in `templates/Dockerfile`, add the new release's SHA256 checksums to the tables there, then rebuild with `vibepod init --rebuild` (the same pin-then-rebuild pattern used for the `codex` CLI — see above). Keep your host's SwiftLint version aligned with the container's (0.65.0): a mismatch changes which lint rules fire, so lint results won't agree between host and container.

**Constraints.** Only Foundation-only, pure SwiftPM packages build and run on Linux. Apple frameworks (UIKit, Vision, Core Image, StoreKit, etc.), `xcodebuild`, and the simulators are not available. Linux's corelibs-foundation differs from Darwin's Foundation in behavior details, so a green run inside the container does not substitute for verification on macOS (host or CI). `lldb` and `swift repl` are unavailable in the container (the debian12 Swift toolchain links `libpython3.11`, which Debian 13 does not ship). `swift build` / `swift test` / `swiftc` / `swiftlint` are unaffected.

**Cache.** SwiftPM's caches (`~/.swiftpm`, `~/.cache/org.swift.swiftpm`, and the module cache) live under the container's home directory, so — like other language toolchains — they persist across `vibepod run` invocations in the default (non-disposable) container. `--worktree` runs use a disposable container and do not retain the cache. The only build artifact left in your workspace is SwiftPM's own `.build/`.

**Network.** Package resolution needs outbound HTTPS. The default container already allows this, so no extra configuration is required — but combining `--no-network` with `profile = "swift"` will cause package resolution to fail.

## Roadmap

VibePod is heading to **v2.0**, where it will be reorganized into a clear pair:

- **vibepod CLI** — a sandbox primitive that safely runs Claude Code (or other agent runtimes) inside Docker containers. No opinions about how you write code.
- **vibepod plugin for Claude Code** — a Claude Code plugin that wraps the CLI and provides opinionated workflows for autonomous tasks.

Until v2.0 is released, no intermediate releases will be cut.

## License

[MIT](LICENSE)
