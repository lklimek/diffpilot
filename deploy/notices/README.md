# Licence notices in the images

diffpilot is licensed under [FSL-1.1-ALv2](../../LICENSE.md). The server and runner image builds generate or fetch
the notices of the third-party software they ship and put them under `/usr/share/doc/diffpilot/`; a notice that
cannot be generated, or a dependency under a licence not accepted here, fails the build. The egress proxy image
holds only Ubuntu packages and diffpilot's configuration files.

| Image | File in `/usr/share/doc/diffpilot/` | Covers | Source |
|---|---|---|---|
| server, runner | `LICENSE.md` | diffpilot itself | repository root |
| server | `third-party-rust-server.txt` | crates linked into `diffpilot-server` | cargo-about, `crates/server` with feature `embed-ui` |
| server | `third-party-npm-web.txt` | production npm packages of the embedded web UI | generate-license-file, `web/package.json` |
| runner | `third-party-rust-runner.txt` | crates linked into `diffpilot-supervisor` and `diffpilot-runner` | cargo-about, `crates/runner` |
| runner | `third-party-rust-analysis.txt` | crates linked into `sr-check` | cargo-about, `crates/analysis` |
| runner | `codex/LICENSE`, `codex/NOTICE` | Codex CLI | `openai/codex` at the pinned release tag |
| runner | `github-mcp-server/LICENSE`, `github-mcp-server/third-party-licenses.linux.md` | github-mcp-server and its Go modules | the release archive and `github/github-mcp-server` at the pinned tag |

Ubuntu packages (all images; Squid in the egress proxy) keep their notices in `/usr/share/doc/<package>/copyright`.
Codex's bundled voice libraries keep theirs in `/opt/codex/codex-resources/voice/` (`NOTICE.md`, `licenses/`).

## Tools

- [cargo-about](https://github.com/EmbarkStudios/cargo-about) 0.9.3, the prebuilt `x86_64-unknown-linux-musl`
  archive verified by SHA-256 (`CARGO_ABOUT_VERSION`, `CARGO_ABOUT_SHA256` in `deploy/server/Dockerfile` and
  `deploy/runner/Dockerfile`; keep both equal). Configuration `about.toml` (accepted licences, target, no dev or
  build dependencies, the workspace's own crates skipped), template `about.hbs`. It runs offline (`--frozen`) after
  `cargo fetch --locked`.
- [generate-license-file](https://github.com/TobyAndToby/generate-license-file) 4.2.5, pinned with its lockfile in
  `npm/` (kept apart from `web/` so the UI's dependency tree does not change).

## Changes

- **A new dependency's licence is refused** (`failed to satisfy license requirements`): check that its terms allow
  redistribution in the images, then add the SPDX identifier to `accepted` in `about.toml`.
- **Bumping a tool:** update the version and the checksum from the release's `.sha256` file (cargo-about), or run
  `npm install --save-exact generate-license-file@<version>` in `npm/`.
- **Bumping Codex or github-mcp-server** (`deploy/runner/Dockerfile`): the notice files are fetched from the same
  tag and verified by `CODEX_LICENSE_SHA256`, `CODEX_NOTICE_SHA256` and `GITHUB_MCP_THIRD_PARTY_SHA256`; a changed
  file fails the build until its checksum is updated.

Local check of the Rust notices without an image build (cargo-about installed):

```sh
cargo about generate --frozen --fail -c deploy/notices/about.toml -m crates/server/Cargo.toml --features embed-ui \
  -o /dev/null deploy/notices/about.hbs
```
