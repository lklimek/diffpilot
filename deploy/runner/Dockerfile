# Runner image (ADR-013, threat model T1/T4): pinned Codex CLI and github-mcp-server, git, ripgrep, the Rust
# supervisor and runner; two uids (runner 10001, agent 10002), no setuid/setgid bits, no package manager, compiler
# or interpreter beyond what Codex needs, no Docker socket. The entrypoint is the root supervisor, started with
# only CHOWN, SETUID, SETGID and KILL (see ADR-013 and deploy/compose.yaml for the run options). Licence and
# third-party notices in /usr/share/doc/diffpilot (deploy/notices/README.md); a notice that cannot be generated or
# fetched fails the build.
#
# Build (from the repository root; the named context `workspace` is the Cargo workspace):
#   docker build --build-context workspace=. -t diffpilot-runner deploy/runner
# Target `base` is the same image before the strip step (the container test adds python3 to it, then strips).

FROM ubuntu:26.04@sha256:d66b366dac61b575bee2aef07e9c26e8070096fbe84a3d4d3eb0da069b5d76db AS cargo-about
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
ARG CARGO_ABOUT_VERSION=0.9.3
# cargo-about-0.9.3-x86_64-unknown-linux-musl.tar.gz.sha256 of release 0.9.3.
ARG CARGO_ABOUT_SHA256=3ad1e94ea1e63d69d1961f4fb15ccbbe9575d906111f96d2cb4dce6df48f71b4
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && curl -fsSL -o /tmp/cargo-about.tar.gz \
        "https://github.com/EmbarkStudios/cargo-about/releases/download/${CARGO_ABOUT_VERSION}/cargo-about-${CARGO_ABOUT_VERSION}-x86_64-unknown-linux-musl.tar.gz" \
    && echo "${CARGO_ABOUT_SHA256}  /tmp/cargo-about.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/cargo-about.tar.gz -C /usr/local/bin --strip-components 1 --no-same-owner \
        "cargo-about-${CARGO_ABOUT_VERSION}-x86_64-unknown-linux-musl/cargo-about"

FROM ubuntu:26.04@sha256:d66b366dac61b575bee2aef07e9c26e8070096fbe84a3d4d3eb0da069b5d76db AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates rustup gcc g++ make cmake perl pkg-config libc6-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# The toolchain pinned by rust-toolchain.toml, in its own layer.
COPY --from=workspace rust-toolchain.toml ./
RUN rustup show active-toolchain || rustup toolchain install
COPY --from=workspace . /src
COPY --from=cargo-about /usr/local/bin/cargo-about /usr/local/bin/cargo-about
# Jobs capped for the shared build host; the target cache is shared with the server image build. Release build
# without features: no test provider. The notices are generated offline from the fetched crates (every platform's, as
# cargo metadata needs them): runner for the supervisor and the runner, analysis for sr-check.
ENV CARGO_BUILD_JOBS=2 \
    CARGO_TARGET_DIR=/build
RUN --mount=type=cache,id=diffpilot-cargo-registry,target=/root/.cargo/registry \
    --mount=type=cache,id=diffpilot-cargo-target,target=/build \
    cargo build --release --locked -p runner -p analysis --bin diffpilot-supervisor --bin diffpilot-runner --bin sr-check \
    && install -D -m 0755 -t /out /build/release/diffpilot-supervisor /build/release/diffpilot-runner /build/release/sr-check \
    && cargo fetch --locked && mkdir -p /out/notices \
    && for crate in runner analysis; do \
        cargo about generate --frozen --fail -c deploy/notices/about.toml -m "crates/$crate/Cargo.toml" \
            -o "/out/notices/third-party-rust-$crate.txt" deploy/notices/about.hbs \
        && test -s "/out/notices/third-party-rust-$crate.txt" || exit 1; \
    done

FROM ubuntu:26.04@sha256:d66b366dac61b575bee2aef07e9c26e8070096fbe84a3d4d3eb0da069b5d76db AS fetch
SHELL ["/bin/bash", "-o", "pipefail", "-c"]

ARG CODEX_VERSION=0.167.0
ARG CODEX_TARGET=x86_64-unknown-linux-musl
# codex-package_SHA256SUMS of release rust-v0.167.0 (S1a).
ARG CODEX_PACKAGE_SHA256=3bbf75e83de6b58aed28f2cf6e33ad423ec31fd4700a1457b94be557ea5a7c6e
ARG GITHUB_MCP_VERSION=2.1.0
# github-mcp-server_2.1.0_checksums.txt, Linux_x86_64 (S5).
ARG GITHUB_MCP_SHA256=a442f9db287cea01008bcb55fe6bc0292270301046c3edbd1a8b22d6e4157b2b
# Notices the release archives lack, from the same tags: Codex LICENSE and NOTICE; github-mcp-server's dependency
# list and the licence texts of its Go modules (third-party/ of the tag's source archive).
ARG CODEX_LICENSE_SHA256=d17f227e4df5da1600391338865ce0f3055211760a36688f816941d58232d8dc
ARG CODEX_NOTICE_SHA256=9d71575ecfd9a843fc1677b0efb08053c6ba9fd686a0de1a6f5382fd3c220915
ARG GITHUB_MCP_THIRD_PARTY_SHA256=7caad4bd6a08cdb9079f5ac9eb2783857569522dfb4b3a52f3d414ff37fb258f
ARG GITHUB_MCP_SOURCE_SHA256=fb85acd573def7a0a5d35739a52da51f1d42fe281e3a044b58776c33d8e0705e

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

RUN mkdir -p /opt/notices/codex /opt/notices/github-mcp-server \
    && raw=https://raw.githubusercontent.com \
    && curl -fsSL -o /opt/notices/codex/LICENSE "$raw/openai/codex/rust-v${CODEX_VERSION}/LICENSE" \
    && curl -fsSL -o /opt/notices/codex/NOTICE "$raw/openai/codex/rust-v${CODEX_VERSION}/NOTICE" \
    && curl -fsSL -o /opt/notices/github-mcp-server/third-party-licenses.linux.md \
        "$raw/github/github-mcp-server/v${GITHUB_MCP_VERSION}/third-party-licenses.linux.md" \
    && curl -fsSL -o /tmp/github-mcp-src.tar.gz \
        "https://codeload.github.com/github/github-mcp-server/tar.gz/refs/tags/v${GITHUB_MCP_VERSION}" \
    && tar -xzf /tmp/github-mcp.tar.gz -C /opt/notices/github-mcp-server --no-same-owner LICENSE \
    && printf '%s  %s\n' "${CODEX_LICENSE_SHA256}" /opt/notices/codex/LICENSE \
        "${CODEX_NOTICE_SHA256}" /opt/notices/codex/NOTICE \
        "${GITHUB_MCP_THIRD_PARTY_SHA256}" /opt/notices/github-mcp-server/third-party-licenses.linux.md \
        "${GITHUB_MCP_SOURCE_SHA256}" /tmp/github-mcp-src.tar.gz \
        | sha256sum -c - \
    && tar -xzf /tmp/github-mcp-src.tar.gz -C /opt/notices/github-mcp-server --no-same-owner --strip-components 1 \
        "github-mcp-server-${GITHUB_MCP_VERSION}/third-party" \
    && test -s /opt/notices/github-mcp-server/LICENSE \
    && test -n "$(find /opt/notices/github-mcp-server/third-party -type f -name 'LICENSE*')" \
    && chmod -R a+rX,go-w /opt/notices

FROM ubuntu:26.04@sha256:d66b366dac61b575bee2aef07e9c26e8070096fbe84a3d4d3eb0da069b5d76db AS base

# The package list (with source packages) is written before strip-image.sh removes the dpkg database.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git ripgrep \
    && rm -rf /var/lib/apt/lists/* \
    && install -d -m 0755 /usr/share/doc/diffpilot \
    && dpkg-query -W -f='${Package} ${Version} ${source:Package} ${source:Version}\n' \
        > /usr/share/doc/diffpilot/packages.txt

COPY --from=fetch /opt/codex /opt/codex
COPY --from=fetch /opt/github-mcp/github-mcp-server /usr/local/bin/github-mcp-server
COPY --from=build /out/diffpilot-supervisor /out/diffpilot-runner /out/sr-check /usr/local/bin/
# Codex's bundled voice libraries keep their notices in /opt/codex/codex-resources/voice.
COPY --from=build /out/notices/ /usr/share/doc/diffpilot/
COPY --from=fetch /opt/notices/ /usr/share/doc/diffpilot/
# After the notices: --chmod on a COPY that creates the directory would apply to the directory too.
COPY --chmod=0644 --from=workspace LICENSE.md /usr/share/doc/diffpilot/LICENSE.md
COPY --chmod=0755 preflight.sh /usr/local/bin/diffpilot-runner-preflight
COPY --chmod=0755 strip-image.sh /usr/local/lib/diffpilot/strip-image.sh
RUN ln -s /opt/codex/bin/codex /usr/local/bin/codex

# No shared group: agent must not read anything group-readable by runner.
RUN groupadd --gid 10001 runner \
    && useradd --uid 10001 --gid 10001 --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin runner \
    && groupadd --gid 10002 agent \
    && useradd --uid 10002 --gid 10002 --no-create-home --home-dir /nonexistent --shell /bin/bash agent \
    && find / -xdev -type f -perm /6000 -exec chmod ug-s {} + \
    && install -d -o 10001 -g 10001 -m 0711 /var/lib/diffpilot-runner /var/lib/diffpilot-runner/codex-homes \
    && install -d -o 10001 -g 10001 -m 0711 /work \
    && chown 10001:10001 /tmp && chmod 0700 /tmp

# Mount points (ADR-013), all Docker volumes on disk: the durable state at /var/lib/diffpilot-runner, and the
# disposable ones at its codex-homes/ (per-job CODEX_HOMEs), /work (checkouts) and /tmp (runner only). A fresh
# volume takes these owners and modes; the supervisor and the runner set them again at every start.
ENV SR_RUNNER_STATE_DIR=/var/lib/diffpilot-runner \
    SR_RUNNER_WORK_DIR=/work \
    SR_RUNNER_CODEX_BIN=/usr/local/bin/codex \
    SR_RUNNER_GITHUB_MCP_BIN=/usr/local/bin/github-mcp-server

# The supervisor must start as root (it drops to runner and agent itself); never set USER here.
ENTRYPOINT ["/usr/local/bin/diffpilot-supervisor", "serve", "--"]
CMD ["/usr/local/bin/diffpilot-runner-preflight", "/usr/local/bin/diffpilot-runner"]

FROM base AS runner
# T4 runner_image_has_no_toolchains: no package manager, no perl; checked again in deploy/runner/r1-test/run.sh.
RUN /usr/local/lib/diffpilot/strip-image.sh
