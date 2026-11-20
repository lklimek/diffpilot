# ADR-021: Dependencies view — deterministic Cargo map, conversion facts, crate notes

- Status: accepted (coordinator, 2026-11-20; owner-accepted goal "Dependencies page in PR analysis", 2026-11-20).
  Brief amendment "Owner-approved amendment — 2026-11-20: Dependencies view"; plan §18; ADR-005 addendum
  2026-11-20.

## Context
LLM fixes tend to be local optima: parsing or conversion of a type is added in the crate the pull request touches
while the type lives in another crate or repository the organisation controls. The reviewer should see per crate
what was added for other crates' types and where those types are defined.

## Decisions
1. **No `cargo` is run.** The server reads `Cargo.toml` files and `Cargo.lock` at the head commit from its bare
   mirror and `analysis::deps` builds the map (pure, no IO): workspaces (`members` globs over the head tree,
   `exclude`, nested workspaces), packages, `[lib] name`, dependency keys with renames, `workspace = true`
   inheritance, target tables; sources from `Cargo.lock` (which already reflects `[patch]`), from the manifests when
   there is no lock file. Running `cargo metadata` would execute PR-controlled configuration (`.cargo/config.toml`)
   in the trusted server, needs a worktree and a toolchain, and resolves over the network without `--no-deps`; the
   runner is no place for server facts. Gaps (`package.workspace` pointers, `cfg` evaluation, build-script output)
   are reported as limits, never guessed.
2. **Conversion facts** come from Rust tokens (`analysis::tech::rust`, rule A6; `TechPlugin` unchanged, call
   hierarchy stays `None`) on added head lines of changed `.rs` files: `impl From/TryFrom<A> for B`, an `impl` of
   any trait for a type of another crate (by the orphan rule only a local trait), and functions named `parse_*`,
   `validate_*`, `from_*`, `try_from_*`, `to_*`, `into_*`, `convert_*`, `decode_*`, `encode_*` whose signature names
   a type of another crate. A type's crate comes from a `use` declaration or a qualified path; glob imports, macro
   bodies and re-exports are not followed (no fact, never a guess). The definition of a type of another crate of the
   same repository is searched with `git grep` at head inside that crate's directory and confirmed by the lexer;
   types of other repositories are `other_repository` (definition not looked up in v1.2).
3. **Crate notes** are optional LLM output of the `plan` job (`PlanOutput.crate_notes`, short keyword notes
   anchored by hunk ids). The server maps hunks to crates and ranks notes by the changed lines of their hunks; the
   view shows the first three per crate and a count of the rest. Invalid notes (unknown hunk, too long, wording) are
   dropped and logged, never a repair round: the plan's coverage must not fail because of them.
4. **Controlled owners** are an installation-wide list in Settings; the pull request's owner is always controlled.
   "Yours" is applied when the view is read, so a Settings change needs no recomputation.
5. **Storage.** The map is the analysis's `dependencies` document (migration 0015 widens the document kinds),
   computed after the pre-pass before the plan, or on first read for older analyses; a mirror read error is not
   stored. The `pr_context` prompt receives the map and the notes as fenced data and is told to look up upstream
   issues and pull requests in controlled dependency repositories (ADR-005 addendum 2026-11-20).
6. **Bounds.** At most 400 manifests and 16 MiB of Cargo text (4 MiB per file), 64 type names and 200 hits per
   definition search, 50 facts per crate in the view. A cut is a listed limit; a non-Cargo repository is
   `not_rust` ("not available"), never "no dependencies".

## Consequences
- A third view "Dependencies" beside Flows and Files (`GET /api/v1/analyses/{id}/dependencies`, R15); Settings
  gains the controlled owners list (`PUT /api/v1/settings/controlled-owners`, R16).
- Facts carry evidence (line, import line, Cargo.lock line, definition line) and badges naming the check; notes are
  labelled LLM; no verdict ("should move") anywhere.
- Follow-ups (plan §18): definitions in other repositories, crates.io repository lookup, newtype and serde-helper
  patterns, markers in the code view, the mint change for private repositories, other ecosystems.
