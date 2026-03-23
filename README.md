# VibePod

Safely run AI coding agents in Docker containers.

VibePod wraps Docker to let you run [Claude Code](https://docs.anthropic.com/en/docs/claude-code) inside an isolated container — with just two commands.

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

Authenticates for container use. Creates a dedicated OAuth session stored in `~/.config/vibepod/auth/credentials.json`. This session is separate from your host's Claude credentials and is used when running containers.

```bash
vibepod login
```

### `vibepod logout`

Removes the shared authentication session. Use `--all` to also remove all isolated container sessions.

```bash
vibepod logout
vibepod logout --all
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
| `--isolated` | Use an isolated auth session for this container (allows multiple containers simultaneously) |
| `--name <name>` | Name for the isolated session (default: `vibepod-<project>-isolated`) |

#### When to use which?

- **`vibepod run`** (interactive) — day-to-day development. You get a normal Claude Code session safely inside a Docker container. Permission prompts work normally — no bypass mode.
- **`--prompt`** (fire-and-forget) — when the spec is already written and you want to kick off autonomous execution with `--dangerously-skip-permissions`. Great for running overnight or during meetings. Pair with a spec file in your repo: `vibepod run --prompt "Follow specs/login.md and implement"`.

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

1. **Docker container** — the agent runs in an ephemeral container, not on your host
2. **Minimal mounts** — only your project directory and Claude auth are mounted; no `~/.ssh`, no `.env`, no home directory
3. **Git safety net** — your project is git-managed, so any unwanted changes can be reverted with `git reset --hard`

This follows [Anthropic's official recommendation](https://docs.anthropic.com/en/docs/claude-code/security) to use `--dangerously-skip-permissions` only inside containers.

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

## Roadmap

| Version | Features |
|---------|----------|
| **v1.0** | `init` + `run` (interactive / fire-and-forget), Claude Code support |
| **v1.1** | Pre-installed plugins (superpowers, frontend-design), `--env-file` with 1Password integration |
| **v1.2** ✅ | `vibepod restore` (git HEAD auto-recovery with session reports) |
| **v2** | Dashboard (Web UI), execution logs, progress monitoring |
| **v2.1+** | Gemini CLI / Codex support, multi-container execution |

## License

[MIT](LICENSE)
