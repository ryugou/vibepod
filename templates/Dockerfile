FROM debian:bookworm-slim

ARG HOST_UID=501
ARG HOST_GID=20

RUN apt-get update && apt-get install -y --no-install-recommends \
  git sudo jq curl ca-certificates gpg \
  && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg | gpg --dearmor -o /usr/share/keyrings/githubcli-archive-keyring.gpg \
  && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" > /etc/apt/sources.list.d/github-cli.list \
  && apt-get update && apt-get install -y --no-install-recommends gh \
  && apt-get purge -y --auto-remove gpg \
  && apt-get clean && rm -rf /var/lib/apt/lists/*

# vibepod ユーザーを作成し、ホストの uid/gid に合わせる
RUN groupadd --non-unique -g ${HOST_GID} vibepod && \
    useradd -m -u ${HOST_UID} -g ${HOST_GID} -s /bin/bash vibepod && \
    echo "vibepod ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers

USER vibepod
ENV PATH=/home/vibepod/.local/bin:$PATH
RUN curl -fsSL https://claude.ai/install.sh | bash

USER root
RUN mkdir -p /workspace && chown vibepod:vibepod /workspace

# codex CLI(musl 静的バイナリ、node/npm 不要)。実行時自動更新はしない。
# 更新は `vibepod init --rebuild` で行う(README 参照)。
RUN set -eu; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
      arm64) codex_arch=aarch64 ;; \
      amd64) codex_arch=x86_64 ;; \
      *) echo "codex: unsupported architecture '$arch'" >&2; exit 1 ;; \
    esac; \
    asset="codex-${codex_arch}-unknown-linux-musl.tar.gz"; \
    url="https://github.com/openai/codex/releases/latest/download/${asset}"; \
    tmp_dir="$(mktemp -d)"; \
    curl -fsSL "$url" -o "${tmp_dir}/${asset}"; \
    tar -xzf "${tmp_dir}/${asset}" -C "${tmp_dir}"; \
    install -m 0755 "${tmp_dir}/codex-${codex_arch}-unknown-linux-musl" /usr/local/bin/codex; \
    rm -rf "${tmp_dir}"
RUN codex --version

USER vibepod
WORKDIR /workspace

CMD ["claude"]
