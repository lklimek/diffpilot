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
   mirror and `analysis::deps` builds the map (pure, no IO) with Cargo's workspace rules: a package's own
   `[workspace]`, its `package.workspace` pointer, else the nearest ancestor root that does not exclude it (an
   excluded package looks further up; a literal `members` entry beats `exclude`, a glob match does not); members are
   the root package, `members` globs over the head tree and path dependencies of members inside the root. Also
   packages, `[lib] name`, dependency keys with renames, `workspace = true` inheritance, `registry` keys, target
   tables; sources from `Cargo.lock` (which already reflects `[patch]`), from the manifests when there is no lock
   file. Running `cargo metadata` would execute PR-controlled configuration (`.cargo/config.toml`)
   in the trusted server, needs a worktree and a toolchain, and resolves over the network without `--no-deps`; the
   runner is no place for server facts. Gaps are reported as limits, never guessed: a package outside the members
   of the workspace above it or with an unreadable `package.workspace` pointer (`workspace_unresolved`), `[target.…]`
   tables listed for every target (`target_specific`, `cfg` is not evaluated). Build-script output does not change
   the dependency graph Cargo files declare and is not read.
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
   computed after the pre-pass before the plan, or on first read for older analyses; a mirror read error
   (including a failed definition search) is not stored, and the next read computes the map again. The `pr_context` prompt receives the map and the notes as fenced data and is told to look up upstream
   issues and pull requests in controlled dependency repositories (ADR-005 addendum 2026-11-20).
6. **Bounds.** At most 400 manifests, 64 lock files (next to a read manifest, read after the manifests) and
   16 MiB of Cargo text (4 MiB per file); per changed file at most 4096 conversion candidates (`source_cut`), item
   headers of at most 512 tokens ending in `{` or `;`; parsing, token rules and definition lookup (each file lexed
   once) run on the blocking pool in a read slot; 64 type names and 200 files read
   per definition search (files found but not read leave their definitions `not_searched`, a listed limit), 50
   facts per crate and 200 crates outside this repository and controlled repositories in the view (the rest are
   counted). The `pr_context` prompt lists at most 5 controlled dependency repositories, 15 facts per crate and
   clips Cargo names. A cut is a listed limit; a non-Cargo repository is
   `not_rust` ("not available"), never "no dependencies".

## Consequences
- A third view "Dependencies" beside Flows and Files (`GET /api/v1/analyses/{id}/dependencies`, R15); Settings
  gains the controlled owners list (`PUT /api/v1/settings/controlled-owners`, R16).
- Facts carry evidence (line, import line, Cargo.lock line, definition line) and badges naming the check; notes are
  labelled LLM; no verdict ("should move") anywhere.
- Follow-ups (plan §18): definitions in other repositories, crates.io repository lookup, newtype and serde-helper
  patterns, markers in the code view, the mint change for private repositories, other ecosystems.
