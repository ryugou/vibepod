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

USER vibepod
WORKDIR /workspace

CMD ["claude"]
