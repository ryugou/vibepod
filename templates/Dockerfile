FROM debian:bookworm-slim

ARG HOST_UID=501
ARG HOST_GID=20

RUN apt-get update && apt-get install -y --no-install-recommends \
  git sudo jq curl ca-certificates \
  && apt-get clean && rm -rf /var/lib/apt/lists/*

# vibepod ユーザーを作成し、ホストの uid/gid に合わせる
RUN groupadd --non-unique -g ${HOST_GID} vibepod && \
    useradd -m -u ${HOST_UID} -g ${HOST_GID} -s /bin/bash vibepod && \
    echo "vibepod ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers

USER vibepod
ENV PATH=/home/vibepod/.local/bin:$PATH
RUN curl -fsSL https://claude.ai/install.sh | bash

# Install Claude Code plugins
RUN claude plugin marketplace add anthropics/claude-code --sparse .claude-plugin plugins && \
    claude plugin marketplace add obra/superpowers-marketplace && \
    claude plugin install superpowers && \
    claude plugin install frontend-design

USER root
RUN mkdir -p /workspace && chown vibepod:vibepod /workspace

USER vibepod
WORKDIR /workspace

CMD ["claude"]
