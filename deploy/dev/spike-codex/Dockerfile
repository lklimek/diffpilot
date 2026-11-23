# Spike Codex image (S1a harness): the runner's pinned Codex CLI in a plain non-root container, the default image of
# scripts/schema-smoke.sh and the deploy/dev smoke scripts. Build it with build.sh, which passes the runner image's
# pins (deploy/runner/Dockerfile); Codex is the official standalone package, verified by SHA-256.
FROM ubuntu:26.04@sha256:d66b366dac61b575bee2aef07e9c26e8070096fbe84a3d4d3eb0da069b5d76db

ARG CODEX_VERSION
ARG CODEX_TARGET=x86_64-unknown-linux-musl
ARG CODEX_PACKAGE_SHA256

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git jq ripgrep bubblewrap \
    && rm -rf /var/lib/apt/lists/*

RUN test -n "${CODEX_VERSION}" && test -n "${CODEX_PACKAGE_SHA256}" \
    && curl -fsSL -o /tmp/codex-package.tar.gz \
        "https://github.com/openai/codex/releases/download/rust-v${CODEX_VERSION}/codex-package-${CODEX_TARGET}.tar.gz" \
    && echo "${CODEX_PACKAGE_SHA256}  /tmp/codex-package.tar.gz" | sha256sum -c - \
    && mkdir -p /opt/codex \
    && tar -xzf /tmp/codex-package.tar.gz -C /opt/codex --no-same-owner \
    && rm /tmp/codex-package.tar.gz \
    && ln -s /opt/codex/bin/codex /usr/local/bin/codex

RUN groupadd --gid 10001 runner \
    && useradd --uid 10001 --gid 10001 --create-home --home-dir /home/runner --shell /bin/bash runner \
    && install -d -o 10001 -g 10001 -m 0700 /codex-home

ENV CODEX_HOME=/codex-home
USER 10001:10001
WORKDIR /home/runner
CMD ["codex", "--version"]
