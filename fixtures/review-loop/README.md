# Review-loop GitHub fixtures (v1.3, spike S13-1)

Synthetic GitHub GraphQL answers for the review loop (`docs/review-workflow-plan.md` §4.1) on the example PR
`acme/node-client#482` (head `52d2da1`, as `fixtures/pr-482`). Bodies follow the thepastaclaw marker grammar recorded
in `docs/spikes/S13-1-thepastaclaw-markers.md`; texts, logins (`alice`, `bob`, `carol-agent`), ids and SHAs are fake.
Format: one `{request, status, response}` recording per file, as `fixtures/pr-482/github` (fake-github).

| File | Query | Content |
|---|---|---|
| `github/pr.json` | Q-L1 | PR facts at the head |
| `github/review-threads.json` | Q-L2 | 8 threads: resolved after a fix with a bot answer (`l1`); open (`l2`); re-raised in place after a withdrawal (`l3`); deferred with a tracking link (`l4`); legacy marker without `dedupe=` (`l5`, outdated); marker with unknown keys (`l6`); human reviewer (`l7`); GitHub `Bot` account (`l8`) |
| `github/reviews.json` | Q-L3 | legacy review without a phase marker (`r0`); three rounds `phase=final` at `3f9a1c2`, `9f3c2e1`, `52d2da1` (`r1` edited in place with a status block and a finding not shown inline; `r2` with a carried item and a follow-up candidate; `r5`); empty bot and agent reviews holding thread replies (`r3`, `r4`); a human `CHANGES_REQUESTED` body with two items (`h1`); a CodeRabbit body (`c1`); a dismissed review (`a1`) |
| `github/comments.json` | Q-L4 | the gate comment (edited in place); a CI state comment; a human PR-level comment; a free-form bot reply; an agent reply |
| `github/probe.json` | Watch probe | two aliases; the second PR does not exist (per-alias `NOT_FOUND`, data for the rest) |
| `github/thread-edits.json` | `userContentEdits` | the edit history of `l3`: created, status block added, re-raised (status removed) |

thepastaclaw is a GitHub `User`, not a `Bot`: the bot rule needs `known_bots` (RQ17).
