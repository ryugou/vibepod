# Security

## Reporting Vulnerabilities

Please report security vulnerabilities via [GitHub Private Vulnerability Reporting](https://github.com/ryugou/vibepod/security/advisories/new). If that is unavailable, open a GitHub Issue with the `security` label at <https://github.com/ryugou/vibepod/issues>.

## Data Transmission

The container communicates with Claude's API as part of normal operation. This is not "offline" — network requests are made by Claude Code inside the container. No additional data is sent to external services by VibePod itself.

**Note on external data transmission:** VibePod mounts host files into the container (`~/.claude/CLAUDE.md`, `~/.claude/skills/`, `~/.claude/agents/`, `~/.claude/plugins/` as read-only, plus `~/.claude/settings.json` when present via a sanitized per-container copy) and injects `GH_TOKEN` when available. If your CLAUDE.md instructions, Claude settings, or any host-side plugins/skills mounted into the container trigger external review tools, repository content may reach additional external services via these credentials and configurations. VibePod itself does not pre-install any plugins inside the Docker image.

**codex auth injection:** When `~/.codex/auth.json` exists on the host, VibePod copies it (and `~/.codex/config.toml`, if present) into a **user-level stage shared by all containers**, `~/.config/vibepod/codex/` (directory `0700`, files `0600`), and mounts it read-write at `/home/vibepod/.codex`, so the bundled `codex` CLI can run inside the container (e.g. for review). This is an allowlist of exactly those two files — `~/.codex/history.jsonl`, `~/.codex/goals_*.sqlite`, and `~/.codex/cache/` are never copied in. The mount is read-write (not read-only) because `codex` rewrites `auth.json` on token refresh; only the staged copy is written to, never the host original. The stage is intentionally **not** per-container: disposable runs (`--new` / worktree) delete their per-container runtime directory on exit, and a per-container stage would destroy an in-container token refresh (the only valid copy once the refresh token has rotated) along with it. **Trade-off:** because the stage is shared, if you run multiple containers concurrently, a token refresh performed by `codex` inside one container becomes visible to the others. This is judged acceptable — `codex` replaces the file wholesale rather than appending, so the practical exposure is limited, and a per-container copy would not have avoided the underlying provider-side refresh-token rotation problem either; consolidating into one stage is the safer overall trade-off. If `auth.json` is absent, VibePod skips the mount and logs a note to stderr — codex review is simply unavailable in that container.

### GH_TOKEN automatic injection

When `gh` is installed and authenticated on the host, VibePod runs `gh auth token` and injects the result as `GH_TOKEN` into the container. If `gh` is not installed or not authenticated, `GH_TOKEN` is not injected. When present, the container process has access to your host GitHub token and can perform GitHub operations (push, create PRs, call GitHub API) with the same permissions as your host user.

**Recommendation:** If your GitHub token has broad repository access, be aware that any code running inside the container (including agent-generated code) can use it. Scope your token to the minimum necessary permissions.

### `op run --no-masking` risk

When `--env-file` references `op://` secrets, VibePod resolves them via 1Password CLI before passing them to the container. If `op run --no-masking` is used or the resolved values appear in container stdout, they may be captured in logs. In shared log environments, treat container stdout as potentially containing resolved secret values.

## Trust Model

### Authentication

OAuth tokens are stored at `~/.config/vibepod/auth/token.json` with `0600` permissions. The OAuth callback opens a browser URL from Claude's auth flow.

### `--mount` trust boundary

`--mount` allows you to mount additional host paths into the container (read-only). The trust boundary is the user who invokes `vibepod run` — VibePod does not validate or restrict which paths can be mounted.

Path traversal or unintended file exposure can occur through misconfiguration (e.g., mounting a directory that contains secrets). Only mount paths you intend the agent to read.

### `vibepod login` network access

`vibepod login` runs a temporary container with `--network host` to complete the OAuth flow. This container has host-level network access for the duration of the login process.

## Container Isolation

See [README.md](README.md) for the 3-layer isolation model (Docker container, minimal mounts, git safety net).
