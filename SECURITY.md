# Security

## Reporting Vulnerabilities

Please report security vulnerabilities via [GitHub Private Vulnerability Reporting](https://github.com/ryugou/vibepod/security/advisories/new). If that is unavailable, open a GitHub Issue with the `security` label at <https://github.com/ryugou/vibepod/issues>.

## Data Transmission

### Standard mode (`vibepod run`)

The container communicates with Claude's API as part of normal operation. This is not "offline" — network requests are made by Claude Code inside the container. No additional data is sent to external services by VibePod itself in standard mode.

**Note on external data transmission:** VibePod mounts host files into the container (`~/.claude/CLAUDE.md`, `~/.claude/skills/`, `~/.claude/agents/`, `~/.claude/plugins/` as read-only, plus `~/.claude/settings.json` when present via a sanitized per-container copy) and injects `GH_TOKEN` when available. If your CLAUDE.md instructions, Claude settings, or any host-side plugins/skills mounted into the container trigger external review tools, repository content may reach additional external services via these credentials and configurations. VibePod itself does not pre-install any plugins inside the Docker image.

### Template mode (`vibepod run --template <name>`)

When `--template <name>` is passed, VibePod mounts `~/.config/vibepod/templates/<name>/` into `/home/vibepod/.claude/` in place of the host mounts described above. Important security notes:

- **Template `settings.json` is NOT sanitized.** Unlike host mode, which strips `hooks` and `statusLine` from the host's `settings.json` via a per-container sanitized copy, template `settings.json` is bind-mounted as-is. A template that ships a malicious or leaky `settings.json` (hooks, statusLine, enabledPlugins pointing at network tools, etc.) will take effect inside the container unchanged. Do **not** put secrets or host-specific paths in a template you share or publish.
- **Template `plugins/`** is mounted as a plain read-only bind. Phase 2 rejects templates that ship an `installed_plugins.json` registry because the absolute `installPath` values cannot resolve inside the container; Phase 3/4 will add a normalized distribution path for template-bundled plugins.
- **Template name validation**: only `[a-zA-Z0-9_-]+` is accepted to block path traversal like `../etc` from escaping `~/.config/vibepod/templates/`.
- `--worktree` and `--template` cannot be used together in Phase 2.

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
