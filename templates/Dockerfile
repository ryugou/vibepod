FROM node:24-slim

ARG HOST_UID=501
ARG HOST_GID=20

RUN apt-get update && apt-get install -y --no-install-recommends \
  git sudo jq curl \
  && apt-get clean && rm -rf /var/lib/apt/lists/*

# ホストの uid/gid に合わせて node ユーザーを再設定（macOS: 501:20）
# --non-unique で既存 GID との衝突を許容（macOS GID 20 は Debian の dialout と重複）
RUN groupmod --non-unique -g ${HOST_GID} node && \
    usermod --non-unique -u ${HOST_UID} -g ${HOST_GID} node && \
    chown -R node:node /home/node

RUN mkdir -p /usr/local/share/npm-global && \
    chown -R node:node /usr/local/share

ENV NPM_CONFIG_PREFIX=/usr/local/share/npm-global
ENV PATH=$PATH:/usr/local/share/npm-global/bin

USER node
ARG CLAUDE_VERSION=latest
RUN npm install -g --loglevel=error @anthropic-ai/claude-code@${CLAUDE_VERSION}

USER root
RUN mkdir -p /workspace && chown node:node /workspace

USER node
WORKDIR /workspace

ENTRYPOINT ["claude"]
