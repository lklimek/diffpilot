# IM4: digest and toolchain name are a pair; never use the stable alias.
FROM rust@sha256:7c7d4eb4d05b7781187a327809466cfddf29753f6defdc3d19a0e291951b7a37
ENV CARGO_BUILD_JOBS=2 RUSTUP_AUTO_INSTALL=0 RUSTUP_TOOLCHAIN=1.100.0-x86_64-unknown-linux-gnu CARGO_HOME=/c2 HOME=/tmp
RUN groupadd -g 10002 worker && useradd -u 10002 -g 10002 -M worker \
 && install -d -o 10002 -g 10002 /c2 /work \
 && find / -xdev -type f -perm /6000 -exec chmod a-s {} + 2>/dev/null
COPY --chmod=0755 deploy/fetch/dp-fetch /usr/local/bin/dp-fetch
USER 10002:10002
WORKDIR /
ENTRYPOINT ["/usr/local/bin/dp-fetch"]
