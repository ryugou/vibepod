# VibePod

Safely run AI coding agents in Docker containers.

VibePod wraps Docker to let you run [Claude Code](https://docs.anthropic.com/en/docs/claude-code) with `--dangerously-skip-permissions` inside an isolated container — with just two commands.

## Quick Start

```bash
# Install (see below for other methods)
brew tap ryugou/tap
brew install vibepod

# Build the Docker image (one-time setup)
vibepod init

# Run interactively inside a safe container
cd your-project
vibepod run

# Or fire-and-forget with a prompt
vibepod run --prompt "Implement the login page"
```

## Commands

### `vibepod init`

Builds the Docker image and creates global configuration. Detects your host UID/GID automatically for seamless file permissions.

### `vibepod run`

Runs an AI coding agent inside a container, mounting your project directory.

| Option | Description |
|--------|-------------|
| *(none)* | **Interactive mode** — opens a Claude Code session inside the container |
| `--prompt "..."` | Fire-and-forget mode — agent runs autonomously and exits when done |
| `--resume` | Continue from the previous session (fire-and-forget) |
| `--no-network` | Disable container networking |
| `--env KEY=VALUE` | Pass environment variables (repeatable) |

#### When to use which?

- **`vibepod run`** (interactive) — day-to-day development. You get a normal Claude Code session, but safely inside a Docker container with `--dangerously-skip-permissions` enabled. Design, implement, and iterate interactively — all without risking your host system.
- **`--prompt`** (fire-and-forget) — when the spec is already written and you want to kick off autonomous execution. Great for running overnight or during meetings. Pair with a spec file in your repo: `vibepod run --prompt "Follow specs/login.md and implement"`.

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
| **v1.0** | `init` + `run` (fire-and-forget), Claude Code support |
| **v1.1** | Interactive mode (default `run`), improved UX |
| **v1.2** | 1Password CLI integration, `vibepod restore` (git HEAD auto-recovery) |
| **v2** | Dashboard (Web UI), execution logs, progress monitoring |
| **v2.1+** | Gemini CLI / Codex support, multi-container execution |

## License

[MIT](LICENSE)
