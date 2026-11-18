# ADR-008: Agent-output shape and its API projection

- Status: accepted (coordinator, 2026-11-16, after spike S7 and the WP1-A plan review; amended 2026-11-16 after
  the WP1-A contract review)
- Source: `docs/spikes/S7-llm-capability.md` (WP1-A schema recommendations); brief §1, §3; plan §5.3;
  `docs/contracts/agent-output.md`.

Flow transport is superseded by [ADR-014](ADR-014-flow-file-output.md); the canonical document and API projection below remain in use.

## Decisions
- **Job kinds** `plan`, `pr_context`, `analyse_flow`, `ask`, one output schema each (`schemas/agent/`).
- **Numbers.** No float, score, confidence, percentage or likelihood anywhere. Integers only for line numbers,
  counters and the plan's `importance_rank`. Severity is the band `low | medium | high` only (ADR-002).
- **plan:** flows with `id` (`f<n>`), title, classification, `importance_rank`, intent, hunk selections
  (`hunk_id` from the server pre-pass, optional head sub-range) and `depends_on`; `unassigned` hunks with a
  reason. Huge PRs: several `plan` jobs over directory batches with the same schema.
- **pr_context:** problem, why needed, what it gives, related work, fit questions with evidence and no `basis`
  (a question is never a fact). The project profile is built by the server (file on the base ref ∪ MemCan,
  file wins) and given in the prompt; its status is a deterministic fact.
- **analyse_flow:**
  - Steps carry a short `title` (step header and diagram label); array order is the step order. Step ids are
    agent-local; the server assigns API ids `"<flow>.<n>"` and annotation ids `"<step>.a<n>"`.
  - The agent emits only graph **edges** (step id → step id, optional branch label); the server derives nodes,
    numbering and node kind from the steps (`added` → added; `context` → unchanged; other kinds → changed).
  - **Locations**. Steps, evidence and pattern sites carry `side: head | base`. `head` is the common
    case (S7: head line ranges work well for the model). `base` is only for removed code without a head line to
    anchor to: a deleted file, an emptied file, a removed block; such a step is `deleted_site` and its lines are
    base lines inside the file's hunks (verify: `BaseSideMisused`). Removed lines next to head lines stay at
    deletion markers inside a head step (`after_line` 0 = before line 1, the last line = at the end of the
    file). Annotations use their step's side; impact references are head-only (code that exists). The plan
    keeps head sub-ranges: a pure-deletion hunk can only be selected whole by its hunk id, and a sub-range covers
    the removed lines whose marker lies inside it. The API carries the side on `StepFacts`, `FileRange(s)`
    (Unexplained, moves), `NotAnalysedItem`, `PatternSite`, `Evidence`, `RangeQuery`/`LinesResponse`/
    `DiffResponse`, `FeedbackTarget::Line` and comment targets; one `LineSide` replaces the GitHub-style
    `left | right` (the server maps base = LEFT, head = RIGHT).
  - Annotation types `behaviour | consequence | question | note`; every annotation carries `basis` and
    evidence. **Every annotation also carries `was` and `is`**: the behaviour of its highlighted
    range before and after, the only content of the hover tooltip (facts, no risk, no verdict) and of the
    inspector's "Behaviour (was → is)" section for that highlight. `text` holds the consequence, question or
    note and is null exactly for `behaviour` annotations (verify: `BehaviourMissing`, `RemarkMismatch`).
    The API `Annotation` is a field-for-field projection (`Annotation::from_agent`). `fact` ⇒ at least one evidence reference is enforced by the server's verify step (the strict
    schema subset has no `if/then`). An impact reference's own location is its evidence.
  - `assessment` is nullable per step (omitted unless a production user or operator would notice something);
    fields: headline, risk band, reason, basis note, not checked, origin, `blocking`, smallest correction.
  - **`blocking: blocks_this_pr | follow_up` is kept** (brief §3 "blocks this PR or follow-up"; mockup "Merge
    impact"). S7 saw `follow_up` on every step only because assessments were mandatory on trivial steps;
    re-measure in WP4-REAL.
  - `moved_from` nullable; `impact_refs`, `pattern_repetitions` kept.
  - `thread_statuses`: the server lists the GitHub threads located in the flow as aliases `t1..tn`; the agent
    returns `still_valid | stale | already_addressed` with a reason. Other threads have no status.
- **ask:** answer text plus evidence references.
- **Derived by the server, not emitted by the agent:** flow risk in the menu = the highest step band (none when
  no step has an assessment); the code-view bar style of an annotated range = the step's band, "affected"
  (dotted) without one; flow group (behaviour, LLM-classified refactor, LLM-classified tests) from the
  classification; tour order; trimmed blank edge lines of step ranges; move verification and badges.
- **Deterministic facts and LLM content are separate API fields** (`facts` / `llm`). PR-context facts include
  `agent_instruction_files` (paths in the diff matching AGENTS.md, CLAUDE.md, `.agents/`, `.codex/`, `.claude/`,
  `.cursor*`, `.github/copilot-instructions.md`; shown as a notice, threat model WP1-T) and
  `related_work_checked` (false when the GitHub App is not installed on the PR repository, ADR-005).
- Whitespace-only changes stay in the hunks given to the agent, marked as a fact. Moves with deterministically
  identical text out of a still existing file into another one (or a pure rename) are quiet rows; their lines stay in
  the agent's hunks as context without coverage duty, and a reorder within one file stays mandatory (brief
  amendment 2026-11-18). An identical move carries no flow
  reference; its row reads "Body text identical" only and never implies unchanged behaviour.
  For the owner: [deviation] brief §4.3 "call site changed in flow N" is not in v1: there is no deterministic
  producer while the call hierarchy is n/a; the LLM sees identical moves only as context.
- `FlowFacts` has no changed-line counter (no consumer in the brief or mockups).

## Consequences
- The agent schemas are generated from `crates/contracts/src/agent/` in the Codex strict subset (ADR-009).
