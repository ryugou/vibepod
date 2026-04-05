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
| `--lang <name>` | Install language toolchain in container (`rust`, `node`, `python`, `go`, `java`). Auto-detected from project files if omitted |
| `--worktree` | Run in an isolated git worktree (requires `--prompt`). Changes are made in `.worktrees/` instead of your working tree |
| `--mount <src:dst>` | Mount additional host path into the container (read-only, repeatable) |
| `--new` | Recreate the container from scratch. Removes a stopped container automatically; if the container is running, stop it first with `vibepod stop` or `vibepod rm` |

**Container reuse is the default.** VibePod creates one container per project (named `vibepod-{project}-{hash}`) and reuses it across runs. Setup only runs once; subsequent `vibepod run` calls skip setup and connect instantly via `docker exec`. Use `--new` to force a fresh container.

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

## Security Model

VibePod provides 3-layer isolation:

1. **Docker container** — the agent runs in an isolated container, not on your host. By default, one container per project is reused across runs; use `--new` or `vibepod rm` to start fresh
2. **Minimal mounts** — only what the agent needs is mounted:
   - `$(pwd)` → `/workspace` (read-write): your project files
   - `~/.claude.json` → container via **temporary copy** (read-write): onboarding state; the host file is never written directly
   - `~/.gitconfig` → `/home/vibepod/.gitconfig` (read-only): git user name and email
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

#### Stream output (`--prompt` mode)

When running with `--prompt`, VibePod streams Claude Code's activity in real-time via `--output-format stream-json`:

```
────────────────────────────────────────────────────────
  │  [assistant] ファイルを確認します。
  │  [tool_use] Read { file_path: "src/main.rs" }
  │  [tool_use] Edit { file_path: "src/main.rs", old_string: "fn main()...", new_string: "fn main()..." }
  │  [tool_use] Bash { command: "cargo check" }
────────────────────────────────────────────────────────

Result:
Implementation complete. All checks pass.

Container stopped (kept for reuse).
```

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

| Version | Features |
|---------|----------|
| **v1.0** | `init` + `run` (interactive / fire-and-forget), Claude Code support |
| **v1.1** | Pre-installed plugins (superpowers, frontend-design), `--env-file` with 1Password integration |
| **v1.2** | `vibepod restore` (git HEAD auto-recovery with session reports) |
| **v1.3** | Slack bridge mode (removed in v1.4), multi-provider LLM formatting |
| **v1.4** | Stream output, `--worktree` isolation, `--lang` toolchain, `vibepod ps`, `vibepod logs`, `vibepod stop`, `--mount`, `--new`, default container reuse, `vibepod rm`, `config.toml` unified config, bridge removal, docker run unification, run.rs split |
| **v2** | Dashboard (Web UI), execution logs, progress monitoring |
| **v2.1+** | Gemini CLI / Codex as agent runtimes, multi-container execution |

## License

[MIT](LICENSE)
