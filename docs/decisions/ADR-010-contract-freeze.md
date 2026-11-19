# ADR-010: Contract freeze (stage 1 gate)

Date: 2026-11-16. Status: accepted (coordinator gate; the owner was away, see `docs/implementation-plan.md` §1).

## Decision
The contracts are frozen at commit `3c37eac` (branch `wp/1-a`, merged into `feat/v1`): `crates/contracts`,
`schemas/**`, `web/src/api/generated/**`, `crates/db/migrations/0001_init.sql`, the frozen repository signatures
in `crates/db`, the seams `JobExecutor`, `LlmProvider`, `TechPlugin` and the `analysis` public signatures,
`docs/contracts/*.md` including the threat model (`65e53bb`).

From here every change to a contract type, schema, route, migration, trait or frozen signature is a change
request approved by the coordinator (plan §4.3).

## Evidence
- Independent review by Codex Astra (xhigh), four rounds; reports in
  `/data/artifacts/smart-review/2026-11-16/reviews/wp1-a{1,2,3,4}-astra.md`. Final verdict: FREEZE, no open
  blocking finding.
- Schema smoke: one real `codex exec --output-schema` per job kind accepted the final schemas
  (`/data/artifacts/smart-review/2026-11-16/schema-smoke/`).

## For the owner: deviations from the brief accepted at the gate
- Lines in files that are not part of the PR diff cannot be commented in v1 (ADR-006).
- An identical move shows "Body text identical" only, without a link to the changed call site (ADR-008).
- The Ask answer is delivered whole when the model turn ends; no token streaming (ADR-004).
- Sub-agents are off and the Codex sandbox is off (ADR-002).
