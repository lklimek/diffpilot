# Fixture PR `acme/node-client#482`

The example pull request of the mockups (`docs/mockups/05-tour.html`, `mockup.js`): "feat(rpc): retry with
backoff on timeout". Every file here is synthetic; accounts, ids and tokens are fake. Consistency and the trust
wording rules are checked by `crates/contracts/tests/fixtures.rs`.

| Path | Content |
|---|---|
| `repo/base/`, `repo/head/` | Source trees of the base and head commits. Both hold the same `.claudius/project-profile.md` (the `repository_file` profile source). Rust sources are stored as `*.rs.txt` so the rustfmt pre-commit check leaves their line layout (fixture data) alone; use `build-repo.sh`, which restores the `.rs` names. |
| `repo/build-repo.sh DEST` | Builds a git repository: `main` = base, `feat/retry-timeouts` and `refs/pull/482/head` = head. Commit ids are reproducible and recorded in `repo/commits.json`. |
| `repo/hunks.json` | `git diff -U0 -M base head` hunks with ids `h1…` (path order, then file order) and the expected pre-pass outcome `prepass`: `agent`, `identical_move`, `not_analysed:<tests\|docs_comments\|generated_mechanical>`. |
| `agent/` | Agent output per job: `plan.json`, `pr_context.json`, `flow-1.json` … `flow-5.json` (schemas in `schemas/agent/`). |
| `github/` | Fake-GitHub answers adapted from the S3/S4 recordings: `{request, status, response}` per file; `rest/`, `graphql/` (pending review incl. the silent `thread: null` cases), `oauth/`. |
| `codex/` | Codex `exec --json` streams per job; `turn.completed.usage` is cumulative per session in manifest order (`manifest.json`: job kind, session started/resumed, thread id, exit code, outcome, expected output fixture). |
| `api-seed/default/` | API responses of the default state, one file per endpoint; `lines/<side>/<path>.json` holds whole files (all under the 2000-line cap): slice the requested range. |
| `api-states/<state>.json` | RFC 7396 merge patches over the seed document `{"<seed path without .json>": response}` for `wrong`, `missed`, `noq`, `noprofile`, `resume`. Arrays are replaced whole (RFC 7396), so `wrong` and `noq` carry the full `tour.flows`. `noprofile` is an API-state fixture only: the repository still holds `.claudius/project-profile.md`. |

The analysis has 5 flows and 17 steps. Steps `f1.1`–`f1.8`, `f2.1`–`f2.2`, `f3.1`–`f3.2`, `f4.1`, `f5.1` are the 14
steps of `mockup.js`; `f1.9` (client field), `f1.10` (`RetryPolicy`, a new file's lines) and `f5.2` (module tree)
exist because every changed line must lie in a step. `f1.8` ends at line 109 (the loop's closing braces).
