# diffpilot server image: web UI built with npm, embedded into the release binary (feature embed-ui only;
# no test features). Runtime: Ubuntu with git (bare mirrors) and CA certificates, non-root uid 10010. Licence and
# third-party notices in /usr/share/doc/diffpilot (deploy/notices/README.md); a notice that cannot be generated fails
# the build.
#
# Build (from the repository root): docker build -f deploy/server/Dockerfile -t diffpilot-server .

FROM node:26-trixie-slim@sha256:e6bedef217677b276c591795e169840edd9096dbf13f29df8ef8d1856b8820bc AS web
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN --mount=type=cache,id=diffpilot-npm,target=/root/.npm npm ci --no-audit --no-fund
# Licence gate (deploy/notices/about.toml's accepted list) and notices of the UI's production npm dependencies, by
# the generator pinned in its own lockfile.
COPY deploy/notices /src/notices
RUN --mount=type=cache,id=diffpilot-npm,target=/root/.npm npm ci --prefix /src/notices/npm --no-audit --no-fund \
    && node /src/notices/npm/check-licenses.mjs . /src/notices/about.toml \
    && mkdir -p /out/notices \
    && /src/notices/npm/node_modules/.bin/generate-license-file --input package.json --ci --no-spinner --eol lf \
        --output /out/notices/third-party-npm-web.txt \
    && test -s /out/notices/third-party-npm-web.txt
COPY web/ ./
# The type check reads the recorded dashboard and GitHub data imported by the browser mocks.
COPY fixtures/dashboard/api /src/fixtures/dashboard/api
COPY fixtures/pr-482/github/graphql/review-threads.json /src/fixtures/pr-482/github/graphql/review-threads.json
# The type check also reads the API schemas the settings help test imports.
COPY schemas/api /src/schemas/api
# And the needs-you cases the server and web share (queue status test).
COPY docs/contracts/fixtures/needs-you-cases.json /src/docs/contracts/fixtures/needs-you-cases.json
RUN npm run build

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
COPY rust-toolchain.toml ./
RUN rustup show active-toolchain || rustup toolchain install
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY tools tools
COPY prompts prompts
COPY schemas schemas
COPY fixtures fixtures
COPY --from=web /src/web/dist web/dist
COPY deploy/notices/about.toml deploy/notices/about.hbs deploy/notices/
COPY --from=cargo-about /usr/local/bin/cargo-about /usr/local/bin/cargo-about
# Jobs capped for the shared build host; the target cache is shared with the runner image build. The notices are
# generated offline from the fetched crates (every platform's, as cargo metadata needs them).
ENV CARGO_BUILD_JOBS=2 \
    CARGO_TARGET_DIR=/build
RUN --mount=type=cache,id=diffpilot-cargo-registry,target=/root/.cargo/registry \
    --mount=type=cache,id=diffpilot-cargo-target,target=/build \
    cargo build --release --locked -p server --features embed-ui --bin diffpilot-server --bin diffpilot-gateway \
    && install -D -m 0755 -t /out /build/release/diffpilot-server /build/release/diffpilot-gateway \
    && cargo fetch --locked && mkdir -p /out/notices \
    && cargo about generate --frozen --fail -c deploy/notices/about.toml -m crates/server/Cargo.toml \
        --features embed-ui -o /out/notices/third-party-rust-server.txt deploy/notices/about.hbs \
    && test -s /out/notices/third-party-rust-server.txt

FROM ubuntu:26.04@sha256:d66b366dac61b575bee2aef07e9c26e8070096fbe84a3d4d3eb0da069b5d76db AS server
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10010 diffpilot \
    && useradd --uid 10010 --gid 10010 --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin diffpilot \
    && install -d -o 10010 -g 10010 -m 0700 /var/lib/diffpilot /var/lib/diffpilot-gateway \
    && install -d -m 0755 /usr/share/doc/diffpilot \
    && dpkg-query -W -f='${Package} ${Version} ${source:Package} ${source:Version}\n' \
        > /usr/share/doc/diffpilot/packages.txt
COPY --from=build /out/diffpilot-server /out/diffpilot-gateway /usr/local/bin/
COPY --from=web /out/notices/ /usr/share/doc/diffpilot/
COPY --from=build /out/notices/ /usr/share/doc/diffpilot/
# After the notices: --chmod on a COPY that creates the directory would apply to the directory too.
COPY --chmod=0644 LICENSE NOTICE /usr/share/doc/diffpilot/
COPY --chmod=0755 deploy/server/healthcheck.sh /usr/local/bin/diffpilot-healthcheck
COPY --chmod=0755 deploy/server/entrypoint.sh /usr/local/bin/diffpilot-server-entrypoint
ENV SR_DATA_DIR=/var/lib/diffpilot \
    SR_LISTEN_ADDR=0.0.0.0:3000 \
    SR_RUNNER_LISTEN_ADDR=0.0.0.0:3001 \
    SR_WEBHOOK_LISTEN_ADDR=0.0.0.0:3002 \
    HOME=/nonexistent
USER 10010:10010
EXPOSE 3000 3001 3002
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --start-interval=1s CMD ["/usr/local/bin/diffpilot-healthcheck"]
ENTRYPOINT ["/usr/local/bin/diffpilot-server-entrypoint"]
