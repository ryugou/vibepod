# VibePod

Safely run AI coding agents in Docker containers.

VibePod wraps Docker to let you run [Claude Code](https://docs.anthropic.com/en/docs/claude-code) with `--dangerously-skip-permissions` inside an isolated container — with just two commands.

## Quick Start

```bash
# Install
cargo install vibepod

# Build the Docker image (one-time setup)
vibepod init

# Run an AI agent in your project
cd your-project
vibepod run --prompt "Implement the login page"
```

## Commands

### `vibepod init`

Builds the Docker image and creates global configuration. Detects your host UID/GID automatically for seamless file permissions.

### `vibepod run`

Runs an AI coding agent inside a container, mounting your project directory.

| Option | Description |
|--------|-------------|
| `--prompt "..."` | Initial prompt for the agent |
| `--resume` | Continue from the previous session |
| `--no-network` | Disable container networking |
| `--env KEY=VALUE` | Pass environment variables (repeatable) |

Either `--prompt` or `--resume` is required.

## Security Model

VibePod provides 3-layer isolation:

1. **Docker container** — the agent runs in an ephemeral container, not on your host
2. **Minimal mounts** — only your project directory and Claude auth are mounted; no `~/.ssh`, no `.env`, no home directory
3. **Git safety net** — your project is git-managed, so any unwanted changes can be reverted with `git reset --hard`

This follows [Anthropic's official recommendation](https://docs.anthropic.com/en/docs/claude-code/security) to use `--dangerously-skip-permissions` only inside containers.

## Alias

VibePod can be aliased as `vp` for convenience:

```bash
vp run --prompt "Fix the failing tests"
```

## Install

```bash
# From source
cargo install vibepod

# Alias (optional)
ln -s $(which vibepod) ~/.local/bin/vp
```

## Roadmap

| Version | Features |
|---------|----------|
| **v1** | `init` + `run`, Claude Code support |
| **v1.1** | 1Password CLI integration, `vibepod restore` (git HEAD auto-recovery) |
| **v2** | Dashboard (Web UI), execution logs, progress monitoring |
| **v2.1+** | Gemini CLI / Codex support, multi-container execution |

## License

[MIT](LICENSE)
