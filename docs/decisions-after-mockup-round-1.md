# Decisions after mockup round 1 (2026-11-16)

Final state after the owner's review of the first mockups, two psychology reviews
(`/data/artifacts/smart-review/2026-11-16/reviews/psych-astra.md`, `psych-fable.md`) and the owner's manual-review
skill (`/data/artifacts/smart-review/2026-11-16/input/review-assist.md`). Where this file and an older text
disagree, this file wins.

## A. Scope of v1 screens

| Screen | v1 |
|---|---|
| Home | yes — PR URL field only |
| Analysis progress | yes |
| Group workspace | yes — the default screen after opening a PR |
| Tour | yes |
| Settings | yes |
| Pending review | no own screen; a small panel in the workspace |
| Overview | dropped |
| Files (tree) | dropped |
| Re-review (delta since my review) | v2 |

Also out of v1: "recent reviews" list (v2); one-click deepen actions "Find all callers / Check test
coverage / Verify suspicion" (future static analysis); local comment drafts (never); PR-level overall risk
and size statistics (dropped with Overview); numeric confidence; settings switch for showing verdicts.

## B. Rules for every screen

1. Use the full page width (the owner works on a 21:9 monitor). No narrow centred column.
2. The PR number in the top bar opens the PR on GitHub in a new tab.
3. Every label (risk level, classification, provenance badge, status) has a tooltip explaining what it means.
4. Top bar shows a counter "groups reviewed k / n". No percentage, no risk weighting.
5. Keep it simple: prefer removing or rewording over adding UI.

## C. Home

- One field (PR URL) and "Analyze". Signed-out state: "Sign in with GitHub".
- Entering the URL of a PR that already has an analysis resumes it: ticks and answers are kept, nothing is
  re-analysed.
- If the PR head moved since the analysis: banner "This analysis is for `abc123`; the PR is now at `def456`"
  with "Re-analyze". v1 starts a fresh analysis and does not carry ticks over (delta view is v2).

## D. Analysis progress

- The whole group card is clickable and opens that group in the workspace.
- Activity = short live lines of what the agent does (stage, file being read, search), errors and usage-limit
  pauses. Raw agent log on demand.
- Pre-pass result is worded "Recognised by file path, no LLM: Tests (4 files), Docs & comments (3 files),
  Cargo.lock".
- A group whose analysis exists is "analysed", not "done" or "ready". Lines the agent failed to analyse are
  listed as "Unexplained — read these yourself", sorted first.
- States: running, paused (usage limit, resumes at HH:MM), failed chunk with retry.

## E. Group workspace

### Layout (revised after mockup round 2: one block at a time)
The owner found the first workspace overloaded. Rule: the screen shows only what is to be read now — ONE
block at a time. Never two steps side by side, on any screen width.

- Three columns: Navigation | Centre | Inspector.
- **The workspace has no diagram and no PR context.** Both live in the Tour (section F).
- **Centre shows one block.** Group selected, no step: the group's problem statement. Step selected: that
  one step — header, the four facts (called by / decides or changes / goes next / why), its code, impact
  outside the diff, collapsed assessment, tick.
- Under the block: "Previous" / "Next" (step order; after the last step, the next group).
- The navigation column lists the steps of the selected group under it.
- The inspector is hidden (a thin strip) until the user clicks a highlight in the code; nothing is pinned
  automatically. Inspector sections are collapsible; the collapsed state is remembered (localStorage).
- The legend is not on the page; it is in the `?` help.
- Column widths are resizable by dragging; sizes are remembered.
- Each column has "hide" and "full screen". A hidden column leaves a strip with its name; clicking anywhere
  on the strip shows it again.

### Navigation column, in this order
1. Flow groups, most important first: title, risk, tick; the selected group shows its numbered steps.
2. Quiet rows, always last and never above a rated group: moves with identical text; "Not analysed" sections
   (Tests, Docs & comments, Mechanical & generated).
3. Pending review: count and link; the list opens in the centre.

### PR context (shown in the Tour, not in the workspace)
- The problem the PR solves and why it is needed.
- Related work found by the agent: duplicates, competing solutions, stacked PRs, linked issues, companion
  repositories — each with a link and one line on the overlap.
- Fit questions for the reviewer, with evidence: right layer / repository? oversized for the problem?
  duplicates an existing mechanism? These are questions to check, not verdicts.
- CI status with the names of failing checks and tests.
- Counters from the deterministic pre-pass: "N lines removed from tests", "M dependencies changed".
- Project profile status: found (source) or "no project profile — ratings are component-level".

### A group is one flow
- A group follows control flow: trigger → calls → state changes → result, across files, including unchanged
  code that connects the changes.
- Group header: local problem statement — trigger, what was wrong or missing, intended correction, a short
  before/after example.
- Flow diagram (Tour only): numbered steps, decisions with labelled branches, loops, error exits.
  Added code, changed code and unchanged context are distinguishable by colour AND by a second signal
  (border style or marker).
- A pattern applied in several places is explained once; the other places are a collapsed row
  "same pattern × N".

### A fragment is one step
- Numbered like its diagram node. Shows file, symbol, link to the line on GitHub (head SHA, new tab).
- Final code only. Every changed line has a neutral gutter mark (deterministic fact). LLM colouring is laid
  on top, only on annotated ranges, so "changed, no LLM remark" is visible as its own category.
- Visible at once: called by, what it decides or changes, where execution goes next, and **Why** (why this
  step or modification is needed).
- Collapsed until clicked: **Assessment** and recommendation.
- Deleted code: the removed lines are shown by default at the deletion marker, with the removed capability
  named ("Removed the single-attempt API"). This is the one exception to "final code only".
- "Show old code" expands an inline classic diff.
- Impact outside the diff stays inline under the fragment, as real code.
- Hover tooltip on a highlight: the was → is fact only. No risk, no verdict.

### Inspector sections, in this order
1. Classic diff of the selected range.
2. Behaviour (was → is).
3. Why.
4. Consequences.
5. Call hierarchy — "Callers found" / "Callees"; every entry links to file and line on GitHub (new tab).
   Stated as a search result, not as a complete list.
6. Questions to check — an LLM suspicion is phrased as a question for the reviewer
   ("Can `broadcast_tx` be sent twice when the reply arrives late?").
7. Assessment (collapsed by default): what a production user or operator would notice; risk level at system
   scale with the reason (also for LOW); basis ("read 2 files, nothing executed") and what was not checked;
   introduced by this PR or pre-existing; blocks this PR or follow-up; smallest correction.
8. GitHub threads at this location, each with an LLM status: still valid / stale / already addressed.
9. Ask — a text field of one line that grows while typing. Answers stay pinned and are labelled as LLM answers.
10. Feedback — correct / wrong / irrelevant / overrated / underrated, optional note.
11. Comment — on ANY displayed line, annotated or not. "Add to GitHub pending review" writes straight into
    the user's pending review on GitHub.

### Wording and provenance
- One line per panel says "LLM analysis unless marked otherwise". Only deterministic facts get a badge, and
  the badge names exactly what was checked: "text identical", "classified by path", "classified by line rule".
- Removed wording: "verified" as a blanket word, "no risk", "nothing to review", "no review needed",
  "noise" (the section is "Not analysed").
- A moved block with identical text reads "Body text identical; call site changed in group 1".
- Green tick only for deterministic content checks.
- No decimals of confidence anywhere.
- When the LLM raises nothing: "The LLM raised no questions. That is not evidence of no defects."

### Three hard cases the mockup must show (as switchable states)
1. A confident but wrong HIGH: the reviewer marks it wrong / overrated.
2. A defect in a changed line without any annotation: the reviewer comments on a plain changed line; it is
   recorded as "missed by the LLM".
3. A PR where the LLM raises no questions.

## F. Tour

- Both views exist in v1: the workspace (Groups) and the Tour. The Tour is read-only: no comments, Ask or
  feedback there in v1.
- **Start screen = PR context.** Entering the Tour for the first time shows the PR context (contents as in
  section E "PR context", plus what the change gives and why it is needed). The left menu has a
  "PR context" link above step 1.
- The flow diagram is shown in the Tour only.
- Steps follow control flow (the same flows as the groups), including deletions.
- No ticks in the tour. A link jumps to the highest-rated question.
- "Not analysed" material is skipped and listed as skipped.

## G. Comments

- A comment goes directly into the user's GitHub pending review (first comment creates it, later ones are
  added; an existing pending review is reused). Edit and delete act on GitHub. Nothing is stored as a local
  draft. The user submits the review on GitHub.
- Expected API: GraphQL `addPullRequestReview` (pending) and `addPullRequestReviewThread`; to be verified at
  implementation.
- A comment on a line without an annotation is also saved as feedback "missed by the LLM".

## H. Settings

- LLM credentials: "working" / "not working" only.
- Language of LLM texts.
- Per repository, read-only: built-in "not analysed" rules (path globs and line regexes), related
  repositories, project profile source.
- GitHub App installation status.

## I. Agent and analysis

1. Groups are flows; the agent emits the flow graph as structured data (nodes, edges, branch labels, kind per
   node), not as diagram syntax. The UI renders it.
2. Review lenses in the prompt: reuse of existing code, right layer / ownership, data design, behaviour at
   boundaries, performance and abuse.
3. Local-optimum control: related work (h) is done entirely by the LLM. The runner has no GitHub token, so
   the server exposes a read-only GitHub lookup (PRs, issues, search, limited to the repository and its
   related repositories) to the agent as an MCP tool, using the user's token on the server side.
4. Risk is rated at system scale against a project profile (MemCan `type: project-profile`, or
   `.claudius/project-profile.md` on the base ref). Every rating opens with what a production user or
   operator would notice.
5. The agent uses the `claudius` plugin directly (severity and system-rating rules, review skills) instead
   of copies of those rules.
6. Feedback is stored locally and written to MemCan; overrated / underrated are saved as severity
   calibration examples and reused by later analyses.
7. "Not analysed" rules: path globs and line regexes (for example lines matching `^\s*//`), built in.
   Custom rules later.
8. Tests are not shown or walked through, but the agent may read them as evidence ("covered by test X").
9. Wording rule for the agent: observed facts carry a line reference; inferences are marked as inferences;
   hedges must survive shortening.
10. Deterministic, free signals surfaced from "Not analysed" material: failing CI tests, number of lines
    removed from tests, number of changed dependencies. No LLM interpretation of that material in v1.

## J. Later

- v2: re-review delta; recent reviews; LLM auto-comment; Claude provider; Rust and Go plugins.
- Later: static-analysis actions (callers, test coverage); sync of ticks with GitHub "Viewed"; custom rules.

## K. Frontend structure

Every block on a screen is a self-contained component that can be placed on another screen without change:
it receives its data and callbacks as inputs and does not know which screen hosts it. Screens only arrange
components. Expected components: navigation list, PR context, flow diagram, step block (four facts + code),
code view (line renderer with gutter marks and highlights), inspector and each of its sections, Ask,
feedback buttons, comment composer, pending-review list. Purpose: adding the inspector and commenting to the
Tour later is a layout change, not a rewrite.

## L. Tour-only variant (candidate for the MVP, mockups in `docs/mockups-tour/`)

The owner considers moving the Groups workspace to a later version and starting with the Tour alone. In this
variant the Tour is the whole review screen and takes over what the workspace did.

- Screens: Home, Analysis progress, Tour, Settings. No Groups / Tour switch in the top bar.
- Sequence: PR context → for each flow: flow introduction → its steps, one at a time → next flow.
- Left menu: PR context; flows in importance order (title, risk, tick), the current flow lists its numbered
  steps; then text-identical moves, "Not analysed" sections and "Unexplained" lines, which open in the centre.
- Centre is split horizontally. Top: the flow diagram, hidden by default; when shown, the current step is
  marked and clicking a node goes to that step. Bottom: one block — the flow introduction or one step
  (header, four facts, code, impact outside the diff, collapsed assessment, show old code, tick).
  Under it: Previous / Next.
- Flow introduction wording: "Problem (before this PR)" and "Change (what this PR does)", then the
  Before / After example. The label "Was wrong" is not used.
- Inspector on the right, a strip until a highlight is clicked (sections and dock as in section E).
- Comments on any displayed line. While a comment is being written its line is highlighted; after posting,
  the line keeps a comment marker. Pending review count in the top bar; the list opens in the centre.
- Ticks per step; a flow is reviewed when all its steps are ticked; top bar "flows reviewed k / n".
  Opening the PR again returns to the last viewed block.
- Carried over unchanged: resizable columns, click-anywhere strip to restore a hidden column, legend in
  `?`, tooltips on labels, neutral deterministic labels, the three hard cases, keyboard.
- Ask (a question to the LLM about the current step, answered by an on-demand agent run) is part of the MVP.
  It keeps the LLM integration honest: the architecture must support interactive, on-demand jobs with
  session context, not only the batch analysis pipeline.
- The inspector is never empty. With no highlight selected it describes the current block: for a step —
  classic diff of the step, behaviour (was → is), why, consequences, call hierarchy, GitHub threads, with
  Questions and Assessment collapsed; for a flow introduction or PR context — the matching summary. Clicking
  a highlight narrows it to that annotation; a "Back to step" link returns. Ask, feedback and comment work
  at step level without selecting a highlight. This replaces "the inspector is empty until a click"; the
  rule "nothing evaluative is shown before the evidence" stays (Assessment collapsed).

## M. Final decisions (2026-11-16, end of design phase) — these win over every section above

1. **MVP = Tour only.** The Groups workspace is not built in v1. Section L describes the review screen;
   the workspace parts of section E apply only where L refers to them (step block, inspector sections,
   wording and provenance, hard cases). Screens of v1: Home, Analysis progress, Tour, Settings.
2. **Analysis progress** is a single centred progress bar: "k of n groups analysed" and "Cancel analysis";
   states running, paused (usage limit, resume time), failed (Retry), finished ("Open review"). The review
   opens only when the analysis has finished; no partial review in v1.
3. **Inspector is never empty** (section L, last bullets). **Ask is in the MVP.**
4. **Unverifiable refactors** (LLM says pure refactor or rename, no deterministic confirmation): shown as a
   normal flow labelled as LLM-classified refactor, ordered after behaviour-changing flows and before quiet
   rows; never collapsed. Deterministic rename / syntax-level confirmation comes with language plugins (v2).
5. **Project profile** = union of `.claudius/project-profile.md` on the base ref and MemCan
   `type: project-profile`; on conflict the repository file wins. Missing both: the analysis runs without
   and the PR context says "no project profile — ratings are component-level".
6. **Related repositories** are auto-detected from `Cargo.toml` / `Cargo.lock` (git and path dependencies),
   limited to repositories of the same GitHub organisation. No manual configuration in v1.
7. **claudius plugin for the agent**: the runner keeps a git checkout of `lklimek/claudius` on a volume,
   refreshes it (`git pull --ff-only`) before a job when the last refresh is older than a configured
   interval, and exposes its skills directory to Codex. Ref configurable. Verified 2026-11-16: Codex CLI
   0.167 accepts `lklimek/agents` as a marketplace but finds no plugins in it (Claude-format manifest only).
8. **ChatGPT plan: Pro** (no 5-hour window). Usage-limit pause and resume must work but is not a first-stage
   priority.
9. **Mockups**: the approved set is `docs/mockups/` (tour-only). Earlier sets are archived outside the
   repository in `/data/artifacts/smart-review/2026-11-16/` (`mockups`, `mockups-v2`, `mockups-v3`).
10. **Frontend components** (section K) are mandatory structure, not a suggestion.
