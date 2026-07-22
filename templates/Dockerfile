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
# 更新は Dockerfile 内の CODEX_VERSION / SHA256 テーブルを bump してから
# `vibepod init --rebuild` で行う(README 参照)。
#
# CODEX_VERSION は https://github.com/openai/codex の "rust-v<version>" タグを
# pin する。既定値のダウンロード元は
# releases/download/rust-v${CODEX_VERSION}/<asset> で、取得後に SHA256 を
# 検証してからでないと展開・インストールしない(supply-chain 対策)。
# `--build-arg CODEX_VERSION=latest` を指定した場合のみ、従来どおり
# releases/latest/download/<asset> から取得し、チェックサム検証はスキップする
# (この場合はビルドログに警告を出す)。
ARG CODEX_VERSION=0.145.0

# pin されたバージョンごとの SHA256(アーキ別)。CODEX_VERSION を bump する際は
# 対応する行をこのテーブルに追加すること。未知の組み合わせは latest 以外では
# ビルド失敗として扱う(取り違え防止のため、どちらのハッシュがどちらの
# アーキ用かを下記コメントで明記する)。
#   codex-aarch64-unknown-linux-musl.tar.gz (arm64) の SHA256:
#     d384f90bc842450b42bd675feef06a12a46a3b1ca97efcb22566b270e4a11227
#   codex-x86_64-unknown-linux-musl.tar.gz (amd64) の SHA256:
#     bfaf13c9ba34f2ad764e4a916c49cf7177aeba329cf0f719e2227566fc8d662a
RUN set -eu; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
      arm64) codex_arch=aarch64 ;; \
      amd64) codex_arch=x86_64 ;; \
      *) echo "codex: unsupported architecture '$arch'" >&2; exit 1 ;; \
    esac; \
    asset="codex-${codex_arch}-unknown-linux-musl.tar.gz"; \
    expected_sha256=""; \
    if [ "$CODEX_VERSION" = "latest" ]; then \
      url="https://github.com/openai/codex/releases/latest/download/${asset}"; \
      echo "codex: CODEX_VERSION=latest — downloading unpinned release, skipping checksum verification" >&2; \
    else \
      url="https://github.com/openai/codex/releases/download/rust-v${CODEX_VERSION}/${asset}"; \
      case "${CODEX_VERSION}:${codex_arch}" in \
        "0.145.0:aarch64") expected_sha256="d384f90bc842450b42bd675feef06a12a46a3b1ca97efcb22566b270e4a11227" ;; \
        "0.145.0:x86_64") expected_sha256="bfaf13c9ba34f2ad764e4a916c49cf7177aeba329cf0f719e2227566fc8d662a" ;; \
        *) echo "codex: no known SHA256 for CODEX_VERSION=${CODEX_VERSION} (arch ${codex_arch}); bump the checksum table in this Dockerfile or build with --build-arg CODEX_VERSION=latest" >&2; exit 1 ;; \
      esac; \
    fi; \
    tmp_dir="$(mktemp -d)"; \
    curl -fsSL "$url" -o "${tmp_dir}/${asset}"; \
    if [ -n "$expected_sha256" ]; then \
      echo "${expected_sha256}  ${tmp_dir}/${asset}" | sha256sum -c -; \
    fi; \
    tar -xzf "${tmp_dir}/${asset}" -C "${tmp_dir}"; \
    install -m 0755 "${tmp_dir}/codex-${codex_arch}-unknown-linux-musl" /usr/local/bin/codex; \
    rm -rf "${tmp_dir}"
RUN codex --version

USER vibepod
WORKDIR /workspace

CMD ["claude"]
