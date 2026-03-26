# Security

## Reporting Vulnerabilities

Please report security vulnerabilities via GitHub Issues at <https://github.com/ryugou/vibepod/issues> with the `security` label, or email the maintainer directly.

## Data Transmission

VibePod operates in two modes with different data flows:

### Standard mode (`vibepod run`)

No external data transmission beyond the Docker container. Container communicates only with Claude's API (via the mounted auth token).

### Bridge mode (`vibepod run --bridge`)

Bridge mode sends data to **three external services**:

| Destination | What is sent | Why |
|---|---|---|
| **Slack** (via Bot/App tokens) | LLM-formatted terminal output excerpts, session start/end notifications | Remote notification when the agent is waiting for input |
| **LLM API** (Anthropic, Google, or OpenAI — selected via `--llm-provider`) | Raw terminal output (ANSI-stripped, last ~3000 chars) + a fixed system prompt | Cleans TUI artifacts before Slack notification |
| **Local disk** (`~/.config/vibepod/bridge-logs/`) | JSONL logs with terminal excerpts and stdin responses | Debugging and audit trail |

**What may leak:**
- Code snippets, file paths, prompts, or secrets that appear in the terminal output may be sent to the selected LLM provider and Slack.
- Each LLM provider has its own data retention and training policy. Review their terms before use.
- Use `--llm-provider none` to disable external LLM calls entirely (local ANSI stripping only).

**Startup disclosure:** VibePod prints a notice at bridge startup listing the active LLM provider and Slack channel.

### Gemini API key transport

The Gemini API uses a query-string `?key=` parameter (Google's official pattern). While functional, this means the API key appears in URLs. Proxy or network logs may capture it. The other providers (Anthropic, OpenAI) send keys via HTTP headers.

## Trust Model

### Slack channel security

In bridge mode, **anyone in the configured Slack channel** can respond to VibePod notifications (button clicks, reactions, thread replies). These responses are sent directly to the container's stdin.

**Recommendation:** Use a **private channel** with restricted membership. A shared public channel allows anyone in the workspace to send input to your container.

### bridge-logs

Log files at `~/.config/vibepod/bridge-logs/*.jsonl` contain terminal output excerpts and stdin input. File permissions are set to `0600` (owner-only). These files may contain sensitive information — treat them accordingly.

### Authentication

OAuth tokens are stored at `~/.config/vibepod/auth/token.json` with `0600` permissions. The OAuth callback opens a browser URL from Claude's auth flow.

## Container Isolation

See [README.md](README.md) for the 3-layer isolation model (Docker container, minimal mounts, git safety net).
