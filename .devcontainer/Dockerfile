FROM node:24-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
  git \
  sudo \
  iptables \
  ipset \
  iproute2 \
  dnsutils \
  jq \
  curl \
  && apt-get clean && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /usr/local/share/npm-global && \
    chown -R node:node /usr/local/share

ENV NPM_CONFIG_PREFIX=/usr/local/share/npm-global
ENV PATH=$PATH:/usr/local/share/npm-global/bin

USER node

# Claude Code インストール
RUN npm install -g @anthropic-ai/claude-code

USER root
RUN mkdir -p /workspace && chown node:node /workspace

USER node
WORKDIR /workspace

ENTRYPOINT ["claude"]
