ARG VIBEPOD_PROFILE=default

FROM debian:bookworm-slim AS base

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

FROM base AS profile-default

FROM base AS profile-swift
USER root

# Swift 実行時依存パッケージ(公式 swiftlang/swift-docker 6.3/debian/12 と同一 +
# zip 展開用の unzip)。base で git 等は導入済みだが、apt-get install は冪等
# なのでこのパッケージリストをそのまま実行する。
RUN apt-get update && apt-get install -y --no-install-recommends \
  binutils libicu-dev libcurl4-openssl-dev libedit-dev libsqlite3-dev \
  libncurses-dev libpython3-dev libxml2-dev pkg-config uuid-dev tzdata git gcc \
  libstdc++-12-dev unzip \
  && apt-get clean && rm -rf /var/lib/apt/lists/*

# Swift toolchain(アーキ別 SHA256 検証必須。codex CLI と同じバージョン pin
# + SHA256 テーブル方式を採用するが、codex と異なり latest エスケープハッチ
# は設けない)。SWIFT_VERSION はホスト Xcode の Swift バージョンと一致させる
# こと。更新は ARG と下記 SHA256 テーブルの両方を bump してから
# `vibepod init --rebuild` で行う(README 参照)。
ARG SWIFT_VERSION=6.3.3

# pin されたバージョンごとの SHA256(アーキ別)。SWIFT_VERSION を bump する際は
# 対応する行をこのテーブルに追加すること。未知の組み合わせはビルド失敗として
# 扱う(latest エスケープハッチは無し)。
#   swift-${SWIFT_VERSION}-RELEASE-debian12-aarch64.tar.gz (arm64) の SHA256:
#     ecba8ef87b54a5048d466af500f3169c939a6b8a2cb7c600f76b5184457f293a
#   swift-${SWIFT_VERSION}-RELEASE-debian12.tar.gz (amd64) の SHA256:
#     19e0c78cad5418ad48bfa87aa20c53ac9ac9996d1695d04dd94f7c7ea4eb133f
RUN set -eu; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
      arm64) suffix="-aarch64" ;; \
      amd64) suffix="" ;; \
      *) echo "swift: unsupported architecture '$arch'" >&2; exit 1 ;; \
    esac; \
    case "${SWIFT_VERSION}:${arch}" in \
      "6.3.3:arm64") expected_sha256="ecba8ef87b54a5048d466af500f3169c939a6b8a2cb7c600f76b5184457f293a" ;; \
      "6.3.3:amd64") expected_sha256="19e0c78cad5418ad48bfa87aa20c53ac9ac9996d1695d04dd94f7c7ea4eb133f" ;; \
      *) echo "swift: no known SHA256 for SWIFT_VERSION=${SWIFT_VERSION} (arch ${arch}); bump the checksum table in this Dockerfile" >&2; exit 1 ;; \
    esac; \
    asset="swift-${SWIFT_VERSION}-RELEASE-debian12${suffix}.tar.gz"; \
    url="https://download.swift.org/swift-${SWIFT_VERSION}-release/debian12${suffix}/swift-${SWIFT_VERSION}-RELEASE/swift-${SWIFT_VERSION}-RELEASE-debian12${suffix}.tar.gz"; \
    tmp_dir="$(mktemp -d)"; \
    curl -fsSL "$url" -o "${tmp_dir}/${asset}"; \
    echo "${expected_sha256}  ${tmp_dir}/${asset}" | sha256sum -c -; \
    mkdir -p /opt/swift; \
    tar -xzf "${tmp_dir}/${asset}" -C /opt/swift --strip-components=1; \
    rm -rf "${tmp_dir}"
ENV PATH=/opt/swift/usr/bin:$PATH

# SwiftLint(アーキ別 SHA256 検証必須。latest エスケープハッチは無し)。
ARG SWIFTLINT_VERSION=0.65.0

#   swiftlint_linux_arm64.zip の SHA256:
#     12d3b84bc5b69ae13a99a5a5c79904f9ce25867f099f6368d0037854f9ee6c26
#   swiftlint_linux_amd64.zip の SHA256:
#     79306a34e5c7cc55a220cd108cbb861dcad5f10138dcdf261e2624ae8b0a486b
RUN set -eu; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
      arm64) swiftlint_arch=arm64 ;; \
      amd64) swiftlint_arch=amd64 ;; \
      *) echo "swiftlint: unsupported architecture '$arch'" >&2; exit 1 ;; \
    esac; \
    case "${SWIFTLINT_VERSION}:${swiftlint_arch}" in \
      "0.65.0:arm64") expected_sha256="12d3b84bc5b69ae13a99a5a5c79904f9ce25867f099f6368d0037854f9ee6c26" ;; \
      "0.65.0:amd64") expected_sha256="79306a34e5c7cc55a220cd108cbb861dcad5f10138dcdf261e2624ae8b0a486b" ;; \
      *) echo "swiftlint: no known SHA256 for SWIFTLINT_VERSION=${SWIFTLINT_VERSION} (arch ${swiftlint_arch}); bump the checksum table in this Dockerfile" >&2; exit 1 ;; \
    esac; \
    asset="swiftlint_linux_${swiftlint_arch}.zip"; \
    url="https://github.com/realm/SwiftLint/releases/download/${SWIFTLINT_VERSION}/swiftlint_linux_${swiftlint_arch}.zip"; \
    tmp_dir="$(mktemp -d)"; \
    curl -fsSL "$url" -o "${tmp_dir}/${asset}"; \
    echo "${expected_sha256}  ${tmp_dir}/${asset}" | sha256sum -c -; \
    unzip -q "${tmp_dir}/${asset}" -d "${tmp_dir}"; \
    install -m 0755 "${tmp_dir}/swiftlint" /usr/local/bin/swiftlint; \
    rm -rf "${tmp_dir}"

RUN swift --version && swiftlint version

USER vibepod

FROM profile-${VIBEPOD_PROFILE}
