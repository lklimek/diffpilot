# ADR-009: Contract generation and drift guard

- Status: accepted (coordinator, 2026-11-16, WP1-A plan review)
- Source: plan §5.1; `docs/spikes/S1a-codex-basics.md` finding 4.

## Decisions
- **Source of truth:** `crates/contracts`. `schemars` and `ts-rs` are normal dependencies of the crate, not
  behind a `ts` feature (gamiscreen's pattern), so the drift test runs under the CI command unchanged
  (`--features runner/fake-provider`). Deviation from plan §5.1 "gamiscreen's ts feature pattern".
- **Generated, checked in:** `schemas/agent/<kind>.schema.json` (agent output, Codex strict subset);
  `web/src/api/generated/types.ts` (every TypeScript type in one file, `TS::decl`) and
  `web/src/api/generated/routes.ts` (the public-listener routes of `contracts::routes::ROUTES`).
- **Drift test** `generated_artifacts_are_current` (`crates/contracts/tests/generated.rs`) regenerates in memory
  and compares byte for byte; `UPDATE_GENERATED=1` rewrites the files. No `#[ts(export)]`: running tests never
  writes files otherwise.
- **Field order.** schemars `preserve_order`: schema property order = field order, which is also the order the
  model generates (evidence and facts before assessment).
- **Codex strict subset** (S1a), applied by `contracts::agent::strict::codex_strict` and checked by a schema
  walker: every object `additionalProperties: false` with every property in `required`; optional = nullable;
  no `oneOf`/`allOf`/`not`/`if`; schemars' `oneOf` of documented unit variants is collapsed to `enum`; `format`
  only `date-time`; local `$ref` only; no `$schema`. The server re-validates agent output with `jsonschema`
  against the same generated schema. Real acceptance: `scripts/schema-smoke.sh` (one real `codex exec` per kind).
- **TypeScript** has no `bigint`: API types use `u32` integers.
