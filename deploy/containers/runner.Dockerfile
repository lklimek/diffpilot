# IM2: Docker API belongs only to launcher mode; no Codex or supervisor is installed.
FROM rust@sha256:7c7d4eb4d05b7781187a327809466cfddf29753f6defdc3d19a0e291951b7a37 AS build
ENV CARGO_BUILD_JOBS=2
WORKDIR /src
COPY . .
RUN --mount=type=cache,id=v13w3-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=v13w3-cargo-target,target=/src/target \
    cargo +1.100.0 build --release --locked -p runner --bin diffpilot-runner \
 && install -D -m 0755 /src/target/release/diffpilot-runner /out/diffpilot-runner
FROM debian:bookworm-slim
ENV CARGO_BUILD_JOBS=2
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates git \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd -g 10002 runner && useradd -u 10002 -g 10002 -M runner \
 && install -d -o 10002 -g 10002 -m 0700 /run/launcher /mirror /var/lib/diffpilot-runner \
 && find / -xdev -type f -perm /6000 -exec chmod a-s {} + 2>/dev/null
COPY --from=build /out/diffpilot-runner /usr/local/bin/
COPY LICENSE.md /usr/share/doc/diffpilot/LICENSE.md
USER 10002:10002
ENTRYPOINT ["/usr/local/bin/diffpilot-runner"]
