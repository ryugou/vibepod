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
| `--timeout <dur>` | Wall-clock limit for a `--prompt` session. Accepts bare seconds (`1800`) or a duration (`30m`, `1h30m`); `0` disables it. Defaults to **30 minutes**. On timeout the container-side agent is stopped, the workspace is restored, and the run exits non-zero |
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
The files are copied (not bind-mounted read-only) into a **user-level stage
shared by all containers** (`~/.config/vibepod/codex/`) and mounted
**read-write** at `/home/vibepod/.codex`, because `codex` rewrites
`auth.json` on token refresh — the same copy-then-mount pattern used for
`~/.claude.json`. The host originals are never touched. The stage is shared
(not per-container) so that disposable runs (`--new` / worktree), which
delete their per-container runtime directory on exit, don't destroy a
container-refreshed `auth.json` along with it; one side effect is that
concurrently running containers share the same staged `auth.json`.

If `~/.codex/auth.json` is missing, VibePod prints a note to stderr and
continues without codex support in that container — this is not a fatal
error.

The `codex` binary itself is **not** auto-updated at runtime (unlike Claude
Code). To pick up a newer `codex` release, rebuild the image with
`vibepod init --rebuild`.

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
   - `~/.codex/auth.json` and `~/.codex/config.toml` (if present) via **temporary copy** (read-write, when `auth.json` exists): written to `~/.config/vibepod/codex/` (shared across all containers, not per-container) and mounted at `/home/vibepod/.codex`; read-write because `codex` rewrites `auth.json` on token refresh
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

If a `--prompt` run exceeds its `--timeout` (default 30 minutes), VibePod stops the container-side agent, restores the workspace to the session's starting commit, prints the `logs.txt` path, and exits non-zero — a timeout is never reported as success.

#### Language toolchain auto-detection

When `--lang` is not specified, VibePod auto-detects the language from project files:

| File | Language |
|------|----------|
| `Cargo.toml` | Rust (+ build-essential) |
| `package.json` | Node.js |
| `go.mod` | Go |
| `pyproject.toml` / `requirements.txt` | Python |
| `pom.xml` / `build.gradle` | Java |

## Roadmap

VibePod is heading to **v2.0**, where it will be reorganized into a clear pair:

- **vibepod CLI** — a sandbox primitive that safely runs Claude Code (or other agent runtimes) inside Docker containers. No opinions about how you write code.
- **vibepod plugin for Claude Code** — a Claude Code plugin that wraps the CLI and provides opinionated workflows for autonomous tasks.

Until v2.0 is released, no intermediate releases will be cut.

## License

[MIT](LICENSE)
