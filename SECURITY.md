# Security

## Reporting Vulnerabilities

Please report security vulnerabilities via [GitHub Private Vulnerability Reporting](https://github.com/ryugou/vibepod/security/advisories/new). If that is unavailable, open a GitHub Issue with the `security` label at <https://github.com/ryugou/vibepod/issues>.

## Data Transmission

The container communicates with Claude's API as part of normal operation. This is not "offline" — network requests are made by Claude Code inside the container. No additional data is sent to external services by VibePod itself.

**Note on external data transmission:** VibePod mounts host files into the container (`~/.claude/CLAUDE.md`, `~/.claude/skills/`, `~/.claude/agents/`, `~/.claude/plugins/` as read-only, plus `~/.claude/settings.json` when present via a sanitized per-container copy) and injects `GH_TOKEN` when available. If your CLAUDE.md instructions, Claude settings, or any host-side plugins/skills mounted into the container trigger external review tools, repository content may reach additional external services via these credentials and configurations. VibePod itself does not pre-install any plugins inside the Docker image.

**codex auth injection:** When `~/.codex/auth.json` exists on the host, VibePod makes it (and `~/.codex/config.toml`, if present) available to the bundled `codex` CLI inside the container (e.g. for review) through two structurally separate areas — this is an allowlist of exactly those two files; `~/.codex/history.jsonl`, `~/.codex/goals_*.sqlite`, and `~/.codex/cache/` are never copied in anywhere:

- **Per-container stage** (`~/.config/vibepod/runtime/<container>/codex/`, directory `0700`, files `0600`): this is the only thing ever mounted into a container, read-write at `/home/vibepod/.codex`. It's read-write because `codex` rewrites `auth.json` on token refresh; only the staged copy is written to, never the host original. Being per-container means anything `codex` writes at runtime under `/home/vibepod/.codex` (session history, sqlite goal databases, cache) is confined to that one container and invisible to any other container running concurrently — there is no cross-container exposure of runtime data. Disposable runs (`--new` / worktree) delete their stage along with the rest of their per-container runtime directory on exit; that's expected, because persistence is handled by the auth store below, not the stage.
- **Host-only auth store** (`~/.config/vibepod/codex-auth/`, directory `0700`, files `0600`): holds the same two allowlisted files and is **never mounted into any container** — only the host-side VibePod process reads or writes it. Before a run starts, VibePod syncs host → store → stage (`auth.json` keep-newest, so an in-container token refresh already reflected in the store isn't clobbered by a stale host copy; `config.toml` always follows the host). After a container stops, and before its per-container runtime directory (and thus its stage) is removed, VibePod syncs any refreshed `auth.json` from the stage back into the store. All copy steps enforce that the source/destination is a regular file (rejecting symlinks and directories without following them) and force `0600` permissions, so a compromised container cannot use the read-write stage mount to make VibePod read or write an arbitrary host path, nor leave a world/group-readable credential behind.

**Accepted risk:** if a container is killed forcibly (`kill -9`, host crash, etc.) before the post-run sync into the auth store runs, an in-container token refresh that only ever reached the stage is lost when that container's runtime directory is cleaned up. The auth store still holds the last value synced from a clean run, so `codex` continues to work; recovery just requires running `codex login` again on the host, after which the next `vibepod run` picks up the fresh `auth.json`.

If `auth.json` is absent, VibePod skips the mount and logs a note to stderr — codex review is simply unavailable in that container.

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
