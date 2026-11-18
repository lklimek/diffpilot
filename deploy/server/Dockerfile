# smart-review server image: web UI built with npm, embedded into the release binary (feature embed-ui only;
# no test features). Runtime: Ubuntu with git (bare mirrors) and CA certificates, non-root uid 10010.
#
# Build (from the repository root): docker build -f deploy/server/Dockerfile -t smart-review-server .

FROM node:26-trixie-slim@sha256:e6bedef217677b276c591795e169840edd9096dbf13f29df8ef8d1856b8820bc AS web
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN --mount=type=cache,id=smart-review-npm,target=/root/.npm npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

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
# Jobs capped for the shared build host; the target cache is shared with the runner image build.
ENV CARGO_BUILD_JOBS=2 \
    CARGO_TARGET_DIR=/build
RUN --mount=type=cache,id=smart-review-cargo-registry,target=/root/.cargo/registry \
    --mount=type=cache,id=smart-review-cargo-target,target=/build \
    cargo build --release --locked -p server --features embed-ui --bin smart-review-server \
    && install -D -m 0755 /build/release/smart-review-server /out/smart-review-server

FROM ubuntu:26.04@sha256:d66b366dac61b575bee2aef07e9c26e8070096fbe84a3d4d3eb0da069b5d76db AS server
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10010 smart-review \
    && useradd --uid 10010 --gid 10010 --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin smart-review \
    && install -d -o 10010 -g 10010 -m 0700 /var/lib/smart-review
COPY --from=build /out/smart-review-server /usr/local/bin/smart-review-server
COPY --chmod=0755 deploy/server/healthcheck.sh /usr/local/bin/smart-review-healthcheck
COPY --chmod=0755 deploy/server/entrypoint.sh /usr/local/bin/smart-review-server-entrypoint
ENV SR_DATA_DIR=/var/lib/smart-review \
    SR_LISTEN_ADDR=0.0.0.0:3000 \
    SR_RUNNER_LISTEN_ADDR=0.0.0.0:3001 \
    HOME=/nonexistent
USER 10010:10010
EXPOSE 3000 3001
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --start-interval=1s CMD ["/usr/local/bin/smart-review-healthcheck"]
ENTRYPOINT ["/usr/local/bin/smart-review-server-entrypoint"]
