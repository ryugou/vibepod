# Security

## Reporting Vulnerabilities

Please report security vulnerabilities via [GitHub Private Vulnerability Reporting](https://github.com/ryugou/vibepod/security/advisories/new). If that is unavailable, open a GitHub Issue with the `security` label at <https://github.com/ryugou/vibepod/issues>.

## Data Transmission

### Standard mode (`vibepod run`)

The container communicates with Claude's API as part of normal operation. This is not "offline" — network requests are made by Claude Code inside the container. No additional data is sent to external services by VibePod itself in standard mode.

**Exception — `--review`:** When `--review codex` or `--review copilot` is used, repository content may reach additional external services. `codex` runs `codex review` inside the container (OpenAI API); `copilot` creates a GitHub PR and triggers GitHub Copilot review (GitHub API). Treat `--review`-enabled runs accordingly.

### Gemini API key transport

The Gemini API uses a query-string `?key=` parameter (Google's official pattern). While functional, this means the API key appears in URLs. Proxy or network logs may capture it. The other providers (Anthropic, OpenAI) send keys via HTTP headers.

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
