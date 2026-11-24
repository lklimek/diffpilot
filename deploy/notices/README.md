# Licence notices in the container images

The current Dockerfiles are authoritative. diffpilot uses [FSL-1.1-ALv2](../../LICENSE.md).

| Image | Included notices |
|---|---|
| server/gateway | `LICENSE.md`, `packages.txt`, `third-party-rust-server.txt` (shared server crate with embed-ui), `third-party-npm-web.txt` under `/usr/share/doc/diffpilot/` |
| runner/launcher | diffpilot `LICENSE.md`; Debian package copyright files under `/usr/share/doc/` |
| worker | diffpilot `LICENSE.md`, Codex `LICENSE`/`NOTICE`/bubblewrap `COPYING` under `/usr/share/doc/diffpilot/`; curated claudius `LICENSE`, `SOURCE`, `SOURCE_COMMIT` under `/etc/codex/claudius/`; Debian package copyright files |
| fetch | Upstream digest-pinned Rust image notices; installed Debian package copyright files |
| egress-proxy | diffpilot licence and Ubuntu package manifest/copyright files |

Runner contains neither Codex, sr-check, github-mcp-server nor a supervisor. Worker carries Codex and sr-check;
its GPL-3.0 claudius tree is pinned to 13.7.0 `71096d7`, with source URL and sibling skill paths. No upstream
schema is vendored into the application crates.

The server build runs cargo-about 0.9.3 (`deploy/notices/about.toml`, `about.hbs`) and the pinned npm licence
generator. Refused licences or failed notice generation stop that build. Worker/runner currently retain package
copyrights but do not generate separate application Rust-dependency notices; completing those manifests is a
release-packaging follow-up, outside W7. Codex's upstream binary also lacks a complete linked-dependency notice
bundle; its supplied LICENSE/NOTICE and bubblewrap COPYING are redistributed. No completeness claim is made.

For Ubuntu sources use the exact `packages.txt` versions at Launchpad or `apt-get source`; Debian sources use
the installed package/version and Debian archive. The image package managers install those binaries unmodified.
Codex notice digests and release archive checksum are pinned in `deploy/worker/Dockerfile`; claudius source is
recorded by `deploy/containers/prepare-claudius.sh`. Update pins and notices together when upgrading.
