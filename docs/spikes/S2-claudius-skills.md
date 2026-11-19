# S2 — claudius skills in Codex

Date: 2026-11-16. Codex CLI 0.167.0. `lklimek/claudius` at `d31993add8b8332ae03352c471be2931c72f7c24`
(main, 2026-11-10). Evidence (sanitised): `/data/artifacts/diffpilot/2026-11-16/spikes/S2/`.

## Question
How does Codex 0.167 discover skills from a git checkout of `lklimek/claudius` at a configurable ref? Is
`severity` actually used?

## Discovery mechanism (source `ext/skills/src/host_roots.rs`, `config/src/skills_config.rs`)
- Skill roots:
  - user: `$HOME/.agents/skills`, and the deprecated `$CODEX_HOME/skills`;
  - system cache: the bundled skills `skill-creator`, `openai-docs`, `imagegen`, `review-agent`;
  - admin: the system config dir;
  - repo: `.agents/skills` from the project root to the cwd;
  - plugin roots.
- `exec` has no config key for an extra root; the app-server sets extra roots through its API.
- Controls: `[skills.bundled] enabled = false` removes the system skills. `[[skills.config]] name|path, enabled`
  toggles single skills. `skills.max_context_tokens` caps the catalog (default 2 % of the context window, at most
  10k). `skills.include_instructions` controls the instruction block.
- The skill format is compatible: claudius `SKILL.md` frontmatter (`name`, `description`) is read as is.
  Relative references between skills resolve against the skill dir.
- The catalog lists name, description and an aliased path (`r0/severity/SKILL.md`). The model opens `SKILL.md`
  with a shell command when it decides to use a skill. Bodies are not injected.

## Runs
| Run | Setup | Result |
|---|---|---|
| `pi-skills` | full `claudius/skills` bind-mounted read-only at `/home/runner/.agents/skills` | all 22 claudius skills plus the bundled system skills in the catalog |
| `s2-implicit` | full set; prompt "review calc.py, rate severity"; effort high | read `coding-best-practices`, `review-changes`, `severity`, `report-format`, then `delegate`, `grand-admiral`, `git-and-github` and references; tried to write `/jobwork/calc-review-tracking.md` (blocked by the read-only fs); spawned a sub-agent; **415k input tokens, 76 s** |
| `s2-curated` | only `severity` + `report-format` mounted; bundled skills off; `multi_agent` off; same task | read both `SKILL.md` and applied severity (likelihood/impact/relevance floats); **65k input tokens, 28 s** |

## Findings
1. Discovery works with no Codex-specific packaging: mount (or copy) a skills directory from a checkout at the
   wanted ref into the agent's `$HOME/.agents/skills`, read-only.
2. **`severity` is used**, both when the skill is named and when the task matches its description.
3. The **full claudius set is harmful for the agent.** Coordinator and workflow skills (`grand-admiral`,
   `delegate`, `review-changes`, `git-and-github`) fire on a review task. They make the agent write tracking
   files, delegate to sub-agents and read many references: about 6× the tokens and 2.7× the time of the curated
   run.
4. `report-format` instructs the claudius report JSON shape and wording. In diffpilot the output shape is our
   `--output-schema`, so `report-format` competes with it and should not be mounted.
5. `severity` produces numeric likelihood/impact/relevance floats. Brief §1 forbids numeric confidence in the
   UI. Either the schema has no place for them, or the server maps them to bands and never renders numbers. This
   needs a contract decision in WP1-A.
6. Skills are only useful if the agent can run shell commands to read them. Sandbox off (S1b) makes this work.
   The skills dir must be readable by uid `agent` and read-only (T1).

## Decision (proposed ADR text)
- **ADR "Agent skills":**
  - The runner keeps a claudius checkout at a configured ref (runner-owned, updated by the runner, never by the
    agent).
  - Per job it exposes a **curated allow-list** of skill directories, read-only, at `$HOME/.agents/skills` of
    uid `agent`. v1 list: `severity` (plus later domain skills such as `security-best-practices` if S7 shows
    value).
  - `[skills.bundled] enabled = false`.
  - Prompts name the skills to apply ("rate each finding per the `severity` skill").
  - Repo skills are excluded by the cwd layout (S1b).
- Configurable values: claudius repo URL, ref, and the allow-list. Recording the resolved commit SHA per analysis
  is useful for reproducibility.
- The fallback ("inject skill text into the prompt") is **not needed**. Injecting is still an option if the
  sandbox-off shell access is ever removed.

## Plan impact
- `JobSpec`: `skills: { ref/commit, names[] }` resolved by the runner to a read-only mount. No skill text
  travels in the spec.
- WP1-A contract: the severity representation (finding 5).
- WP3-RUN: checkout management, per-job skills mount, `skills.bundled.enabled=false`, `multi_agent=false`.
