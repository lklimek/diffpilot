# Dashboard world (synthetic)

Served by `tools/fake-github --dashboard-fixture-dir fixtures/dashboard/github` (format: `tools/fake-github/README.md`,
"Dashboard world"). Invented data only: organisation `acme`, logins `alice` (the fake's signed-in user),
`alice-helper` (linked login that search refuses), `renata-s`, `mira-k`, `jun-park`, `tobias-w`, `olek-d`, `dana`,
`dependabot[bot]`. Dates are fixed around 2026-11-18; in-process tests use that day as "now" (12-week window from 2026-08-26), and
the dev stack serves the world moved to today (`--dashboard-world-at 2026-11-18T19:30:00Z`).

| Case | Items |
|---|---|
| Authored, open | `acme/wallet-ui#311` (CI failing, 2 unresolved threads, changes requested, milestone v2.5), `#298` (stacked on #311's head, team review request; no review decision, merge state `CLEAN`: ready to merge), `acme/node-client#476` (awaiting `tobias-w`), `#490` (draft), `acme/platform#1176` (ready label, awaiting `jun-park` + team) |
| Review requested | `acme/platform#1203` (fixes #1187, CI green, milestone v2.4, owned path), `#1199` (large, milestone v3.0), `acme/wallet-ui#320` (requested today), `acme/node-client#482` (the recorded PR; head follows `POST /_fake/head`), `#495` (excluded bot author), `acme/docs-site#77`, `acme/consensus#1534` (3 unresolved threads) |
| Assigned PR, private | `acme/ops-runbooks#14` (private repository: needs a token) |
| Archived | `acme/legacy-wallet#9` (excluded by `archived:false`) |
| Fork | `acme/docs-site#80` (`isCrossRepository`, head `main` of the fork: not a base for `#77`) |
| Linked login (unsearchable) | `acme/consensus#1541` (author `alice-helper`, stacked on #1534), issue `#1525` (assignee `alice-helper`), events of `alice-helper` |
| Merged, linked login | authored by `alice-helper`: `acme/platform#1231` (merged by `alice`), `acme/consensus#1536` (merged by `mira-k`, absent from the feed), `acme/platform#1233` (merged by `renata-s`, files outside `alice`'s owned paths), `acme/consensus#1501` (older), `acme/review-policies#7` (opened 2026-11-17, merged 2026-11-18 by `alice` in a repository only enumerated; only its `opened` event is in the feed). The feed reports merges as action `merged` with GitHub's slim payload |
| Issues | `acme/platform#1187` (assigned, production report, mention in a comment), `#1210` (assigned), `#1215` (watched repository only), `#1150` (closed), `acme/consensus#1520` (assigned, "Blocked by #1534"), `acme/wallet-ui#305` (mention) |
| History | merged `acme/platform#1190`, closed unmerged `acme/wallet-ui#300`, reviewed by `alice`: `acme/platform#1195` (two reviews), `acme/consensus#1510`; outside the window: `acme/platform#1100` |
| REST | releases (prerelease and draft skipped by `latest`; none for docs-site), milestones (open/closed), ownership policy `acme/review-policies:policies/platform.json`, files of #1203, #1199, #1176, #311 |
| Invisible | `acme/secret` (absent: reach diagnostic and "cannot be searched") |

Open pull requests are `mergeStateStatus` `BLOCKED` (drafts `DRAFT`) unless noted; `viewerPermission` is `WRITE`
except `READ` on `acme/legacy-wallet`.
