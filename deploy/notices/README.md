# Licence notices in the images

diffpilot is licensed under [FSL-1.1-ALv2](../../LICENSE.md). Every image build puts diffpilot's licence, the list of
its Ubuntu packages and the notices of the third-party software it ships under `/usr/share/doc/diffpilot/`. A notice
that cannot be generated or fetched, or a Rust crate or production npm package under a licence that `about.toml` does
not accept, fails the build. The release workflow checks that every file below is present and non-empty.

| Image | File in `/usr/share/doc/diffpilot/` | Covers | Source |
|---|---|---|---|
| all | `LICENSE.md` | diffpilot itself | repository root |
| all | `packages.txt` | Ubuntu packages: `package version source-package source-version` | `dpkg-query` at build time |
| server | `third-party-rust-server.txt` | crates linked into `diffpilot-server` | cargo-about, `crates/server` with feature `embed-ui` |
| server | `third-party-npm-web.txt` | production npm packages of the embedded web UI | generate-license-file, `web/package.json` |
| runner | `third-party-rust-runner.txt` | crates linked into `diffpilot-supervisor` and `diffpilot-runner` | cargo-about, `crates/runner` |
| runner | `third-party-rust-analysis.txt` | crates linked into `sr-check` | cargo-about, `crates/analysis` |
| runner | `codex/LICENSE`, `codex/NOTICE` | Codex CLI itself (see the gap below) | `openai/codex` at the pinned release tag |
| runner | `github-mcp-server/LICENSE`, `github-mcp-server/third-party-licenses.linux.md`, `github-mcp-server/third-party/` | github-mcp-server, the list of its Go modules and their licence texts | the release archive and the source archive of the pinned tag |

Ubuntu packages keep their copyright files in `/usr/share/doc/<package>/copyright` (the runner image keeps them,
although it removes the dpkg database). Codex's bundled voice libraries keep their notices in
`/opt/codex/codex-resources/voice/` (`NOTICE.md`, `licenses/`).

## Source of the Ubuntu packages

`packages.txt` names every Ubuntu binary package with its version and the source package it was built from. Ubuntu
publishes the corresponding source of exactly those versions, GPL-licensed packages such as Squid, git and bash
included: `apt-get source <source-package>=<source-version>` on an Ubuntu system with source repositories enabled, or
`https://launchpad.net/ubuntu/+source/<source-package>/<source-version>`. The images install packages from the Ubuntu
archive unmodified. Whether the project also makes a written offer for that source is the owner's decision (not made
here).

## Known gap: Codex's own dependencies

The Codex CLI binaries (`/opt/codex/bin/`) are upstream's release build, redistributed unmodified. They link many
third-party Rust crates, and upstream ships no notice file for them (neither in the release archive nor at the
tag; its `NOTICE` covers only Ratatui). Generating one with cargo-about from `codex-rs/Cargo.lock` at the pinned tag
is not practical in the image build: its git dependency `rusty_v8` makes `cargo fetch` clone V8 and Chromium
submodules (more than 3 GB, still running after 7 minutes in a trial). The same applies to the `rg` and `bwrap`
binaries in the Codex package. The image therefore carries Codex's `LICENSE` and `NOTICE` only, matching upstream's
own distribution; closing the gap needs a notice file from upstream.

## Tools

- [cargo-about](https://github.com/EmbarkStudios/cargo-about) 0.9.3, the prebuilt `x86_64-unknown-linux-musl`
  archive verified by SHA-256 (`CARGO_ABOUT_VERSION`, `CARGO_ABOUT_SHA256` in `deploy/server/Dockerfile` and
  `deploy/runner/Dockerfile`; keep both equal). Configuration `about.toml` (accepted licences, target, no dev or
  build dependencies, the workspace's own crates skipped), template `about.hbs`. It runs offline (`--frozen`) after
  `cargo fetch --locked`.
- `npm/check-licenses.mjs`: the licence gate of the web UI's production npm packages against the same `accepted`
  list of `about.toml` (fails closed: a missing or unparsable licence, or a `WITH` exception, fails;
  `node npm/check-licenses.mjs --self-test` tests the expression logic).
- [generate-license-file](https://github.com/TobyAndToby/generate-license-file) 4.2.5, pinned with its lockfile in
  `npm/` (kept apart from `web/` so the UI's dependency tree does not change).

## Changes

- **A new dependency's licence is refused** (`failed to satisfy license requirements` from cargo-about, or
  `check-licenses: ... is not accepted`): check that its terms allow redistribution in the images, then add the SPDX
  identifier to `accepted` in `about.toml` (one list for Rust and npm).
- **Bumping a tool:** update the version and the checksum from the release's `.sha256` file (cargo-about), or run
  `npm install --save-exact generate-license-file@<version>` in `npm/`.
- **Bumping Codex or github-mcp-server** (`deploy/runner/Dockerfile`): the notice files and github-mcp-server's
  source archive are fetched for the same tag and verified by `CODEX_LICENSE_SHA256`, `CODEX_NOTICE_SHA256`,
  `GITHUB_MCP_THIRD_PARTY_SHA256` and `GITHUB_MCP_SOURCE_SHA256`; a changed file fails the build until its checksum
  is updated.

Local check of the Rust notices without an image build (cargo-about installed):

```sh
cargo about generate --frozen --fail -c deploy/notices/about.toml -m crates/server/Cargo.toml --features embed-ui \
  -o /dev/null deploy/notices/about.hbs
```
