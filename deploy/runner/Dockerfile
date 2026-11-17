# Runner image skeleton (ADR-013, threat model T1/T4): pinned Codex CLI and github-mcp-server, git, ripgrep,
# two uids (runner 10001, agent 10002), no setuid/setgid bits, no Docker socket. The entrypoint is the root
# supervisor, started with only CHOWN, SETUID, SETGID and KILL (see ADR-013 for the run options).
#
# Build (from the repository root; the named context `workspace` is the Cargo workspace):
#   docker build --build-context workspace=. -t smart-review-runner deploy/runner

FROM ubuntu:26.04@sha256:d66b366dac61b575bee2aef07e9c26e8070096fbe84a3d4d3eb0da069b5d76db AS build
# rustup installs the toolchain pinned by rust-toolchain.toml; jobs capped for the shared build host.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates rustup gcc g++ make cmake perl pkg-config libc6-dev \
    && rm -rf /var/lib/apt/lists/*
COPY --from=workspace . /src
WORKDIR /src
ENV CARGO_BUILD_JOBS=2 \
    CARGO_TARGET_DIR=/build
RUN cargo build --release --locked -p runner --bin smart-review-supervisor --bin smart-review-runner

FROM ubuntu:26.04@sha256:d66b366dac61b575bee2aef07e9c26e8070096fbe84a3d4d3eb0da069b5d76db AS fetch
SHELL ["/bin/bash", "-o", "pipefail", "-c"]

ARG CODEX_VERSION=0.167.0
ARG CODEX_TARGET=x86_64-unknown-linux-musl
# codex-package_SHA256SUMS of release rust-v0.167.0 (S1a).
ARG CODEX_PACKAGE_SHA256=3bbf75e83de6b58aed28f2cf6e33ad423ec31fd4700a1457b94be557ea5a7c6e
ARG GITHUB_MCP_VERSION=2.1.0
# github-mcp-server_2.1.0_checksums.txt, Linux_x86_64 (S5).
ARG GITHUB_MCP_SHA256=a442f9db287cea01008bcb55fe6bc0292270301046c3edbd1a8b22d6e4157b2b

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL -o /tmp/codex.tar.gz \
        "https://github.com/openai/codex/releases/download/rust-v${CODEX_VERSION}/codex-package-${CODEX_TARGET}.tar.gz" \
    && echo "${CODEX_PACKAGE_SHA256}  /tmp/codex.tar.gz" | sha256sum -c - \
    && mkdir -p /opt/codex \
    && tar -xzf /tmp/codex.tar.gz -C /opt/codex --no-same-owner \
    && curl -fsSL -o /tmp/github-mcp.tar.gz \
        "https://github.com/github/github-mcp-server/releases/download/v${GITHUB_MCP_VERSION}/github-mcp-server_Linux_x86_64.tar.gz" \
    && echo "${GITHUB_MCP_SHA256}  /tmp/github-mcp.tar.gz" | sha256sum -c - \
    && mkdir -p /opt/github-mcp \
    && tar -xzf /tmp/github-mcp.tar.gz -C /opt/github-mcp --no-same-owner github-mcp-server \
    && chmod -R go-w /opt/codex /opt/github-mcp

FROM ubuntu:26.04@sha256:d66b366dac61b575bee2aef07e9c26e8070096fbe84a3d4d3eb0da069b5d76db AS runner

# TODO(WP3-OPS phase 2): remove apt/dpkg and other tools beyond Codex's needs (T4 runner_image_has_no_toolchains).
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git ripgrep \
    && rm -rf /var/lib/apt/lists/*

COPY --from=fetch /opt/codex /opt/codex
COPY --from=fetch /opt/github-mcp/github-mcp-server /usr/local/bin/github-mcp-server
COPY --from=build --chmod=0755 /build/release/smart-review-supervisor /build/release/smart-review-runner /usr/local/bin/
RUN ln -s /opt/codex/bin/codex /usr/local/bin/codex

# No shared group: agent must not read anything group-readable by runner.
RUN groupadd --gid 10001 runner \
    && useradd --uid 10001 --gid 10001 --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin runner \
    && groupadd --gid 10002 agent \
    && useradd --uid 10002 --gid 10002 --no-create-home --home-dir /nonexistent --shell /bin/bash agent \
    && find / -xdev -type f -perm /6000 -exec chmod ug-s {} + \
    && install -d -o 10001 -g 10001 -m 0711 /var/lib/smart-review-runner /var/lib/smart-review-runner/codex-homes \
    && install -d -o 10001 -g 10001 -m 0711 /work

# Mount points (ADR-013): state volume at /var/lib/smart-review-runner, a bounded persistent filesystem at its
# codex-homes/ (per-job CODEX_HOMEs, separate from the session store, T8b), size-capped tmpfs at /work (checkouts)
# and at /tmp (runner only, mode 0700).
ENV SR_RUNNER_STATE_DIR=/var/lib/smart-review-runner \
    SR_RUNNER_WORK_DIR=/work \
    SR_RUNNER_CODEX_BIN=/usr/local/bin/codex \
    SR_RUNNER_GITHUB_MCP_BIN=/usr/local/bin/github-mcp-server

# The supervisor must start as root (it drops to runner and agent itself); never set USER here.
ENTRYPOINT ["/usr/local/bin/smart-review-supervisor", "serve", "--"]
CMD ["/usr/local/bin/smart-review-runner"]
