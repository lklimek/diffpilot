# Waiting cases, 2026-11-23

Trimmed from the owner's REST/GraphQL and PR Hygiene captures in
`/data/artifacts/diffpilot/2026-11-23/waiting-cases/`.

Each file contains the captured PR Hygiene record and the equivalent Q2 detail input.
Reviews keep the last 20 entries and their full count, as Q2 does; comments keep the last 30
authors and dates. Checks retain states, names, workflow names and times. Thread state and
comment authors/dates are retained. Review requests have no captured timeline event times.
Bodies, commit messages, contact details, avatars and unrelated API metadata are omitted.
The unused aggregate rollup state is derived from the captured contexts.
Unused required Q2 fields use empty values; base repository write access/default branch are
fixture context and do not decide these cases (all three have merge state BLOCKED).
The fake service's generation time follows the test clock; captured event times stay intact.

- rust-dashcore#1336: ready-for-human; requested human reviewers include old objectors;
  one unresolved bot thread. A current PR Hygiene statement names the reviewers' next move.
- dash-evo-tool#1230 and #1231: configuration-error, blocked stage, no requested reviews;
  PR Hygiene status ERROR and PastaClaw review IN_PROGRESS are ignored by default (K12).
  A configuration error does not establish that maintainers owe a review.
