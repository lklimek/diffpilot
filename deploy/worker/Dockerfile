# IM3: the checker is compiled against this image's Debian ABI (S14-5 P1).
# Named context claudius must be the archive prepared by prepare-claudius.sh.
FROM rust@sha256:7c7d4eb4d05b7781187a327809466cfddf29753f6defdc3d19a0e291951b7a37 AS checker
ENV CARGO_BUILD_JOBS=2
WORKDIR /src
COPY . .
RUN --mount=type=cache,id=v13w3-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=v13w3-cargo-target,target=/src/target \
    cargo +1.100.0 build --release --locked -p analysis --bin sr-check \
 && install -D -m 0755 /src/target/release/sr-check /out/sr-check

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc AS codex
ENV CARGO_BUILD_JOBS=2
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
ARG CODEX_VERSION=0.171.0
ARG CODEX_SHA256=8391d67757496bfd14893eca6cbf0c96994bf2627566ff1f018f70600c9ef6b3
RUN curl -fsSL "https://github.com/openai/codex/releases/download/rust-v${CODEX_VERSION}/codex-package-x86_64-unknown-linux-musl.tar.gz" -o /tmp/codex.tar.gz \
 && echo "${CODEX_SHA256}  /tmp/codex.tar.gz" | sha256sum -c - \
 && mkdir /opt/codex && tar -xzf /tmp/codex.tar.gz -C /opt/codex --no-same-owner \
 && mkdir -p /opt/notices/bubblewrap \
 && curl -fsSL "https://raw.githubusercontent.com/openai/codex/rust-v${CODEX_VERSION}/LICENSE" -o /opt/notices/LICENSE \
 && curl -fsSL "https://raw.githubusercontent.com/openai/codex/rust-v${CODEX_VERSION}/NOTICE" -o /opt/notices/NOTICE \
 && curl -fsSL "https://raw.githubusercontent.com/openai/codex/rust-v${CODEX_VERSION}/codex-rs/vendor/bubblewrap/COPYING" -o /opt/notices/bubblewrap/COPYING \
 && echo 'd17f227e4df5da1600391338865ce0f3055211760a36688f816941d58232d8dc  /opt/notices/LICENSE' | sha256sum -c - \
 && echo '9d71575ecfd9a843fc1677b0efb08053c6ba9fd686a0de1a6f5382fd3c220915  /opt/notices/NOTICE' | sha256sum -c - \
 && echo 'b7993225104d90ddd8024fd838faf300bea5e83d91203eab98e29512acebd69c  /opt/notices/bubblewrap/COPYING' | sha256sum -c -

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc
ENV CARGO_BUILD_JOBS=2 CLAUDE_PLUGIN_ROOT=/etc/codex/claudius
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates git gh ripgrep coreutils python3 python3-jsonschema jq \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd -g 10002 worker && useradd -u 10002 -g 10002 -M worker \
 && install -d -o 10002 -g 10002 /work /work/home /work/codex /work/out /work/pr-data /run/dp /codex /home/worker /cargo /mirror
COPY --from=codex /opt/codex /opt/codex
COPY --from=codex /opt/notices /usr/share/doc/diffpilot/codex
COPY --from=checker /out/sr-check /usr/local/bin/sr-check
COPY --from=claudius / /etc/codex/claudius/
COPY deploy/worker/config.toml /etc/codex/config.toml
COPY deploy/worker/gitconfig /etc/gitconfig
COPY deploy/worker/config.toml deploy/worker/worker-config.py /usr/local/bin/
COPY --chmod=0755 deploy/worker/dp-worker-entry deploy/worker/gh /usr/local/bin/
COPY LICENSE NOTICE /usr/share/doc/diffpilot/
RUN ln -s /opt/codex/bin/codex /usr/local/bin/codex \
 && ln -s /etc/codex/claudius/skills /etc/codex/skills \
 && test "$(cat /etc/codex/claudius/SOURCE_COMMIT)" = 71096d7d4481977added8761b2cfd63cd84852b4 \
 && jq -e '.properties.schema_version.enum | index("4.2.0")' /etc/codex/claudius/schemas/review-report.schema.json \
 && python3 /etc/codex/claudius/scripts/validate_report.py /etc/codex/claudius/tests/fixtures/reports/v4-comment-assessments-host.json \
 && chmod -R a+rX,go-w /etc/codex /opt/codex \
 && find / -xdev -type f -perm /6000 -exec chmod a-s {} + 2>/dev/null
USER 10002:10002
WORKDIR /work
ENTRYPOINT ["/usr/local/bin/dp-worker-entry"]
CMD ["cs1"]
