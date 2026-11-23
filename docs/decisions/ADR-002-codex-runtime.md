# ADR-002: Codex runtime — model, per-job configuration, sandbox, repository isolation, sub-agents, skills

- Status: accepted (coordinator, 2026-11-16, after spikes S1a, S1b, S2, S7)
- Source: `docs/spikes/S1a-codex-basics.md`, `S1b-codex-sessions-isolation.md`, `S2-claudius-skills.md`,
  `S7-llm-capability.md`; plan §5.2–§5.4; brief §5.

## Decisions
- **Model (owner decision, 2026-11-17).** `select_model` remains the single plain Rust selector. It uses
  the installation-wide Settings choice, falling back to `gpt-6.1-sol`, effort `high`. The fixed S1a catalog
  is `gpt-6.1-sol`, `gpt-6-sol`, `gpt-5.6-sol`, `gpt-6-astra`; efforts are `low`, `medium`, `high`.
  Any signed-in allowlisted user can save the choice with CSRF protection. A singleton row records the
  choice, changer login and time; each submission reads it, including later jobs of running analyses.
  Already submitted jobs keep their immutable spec. The runner passes its model and effort to Codex;
  the Tour lists distinct choices from claimed job specs, including Ask and repair jobs.
- **Per-job configuration.** Each job gets a fresh `CODEX_HOME` outside `/tmp` (Codex refuses helper aliases
  there) holding a generated lean `config.toml` (S1a: analytics off; plugins, apps, remote plugins, image
  generation, browser/computer use, hooks, tool suggestions, goals and daemon auto-start off), a copy of the
  credential (ADR-003) and this analysis's session files (ADR-004).
- **Codex sandbox off** (`sandbox_mode = "danger-full-access"`, `approval_policy = "never"`). bubblewrap needs
  `seccomp=unconfined` and `apparmor=unconfined` in the container, which weakens T4 more than the sandbox adds.
  T1–T4 (separate uid, scrubbed environment, `cap_drop: ALL`, `no-new-privileges`, read-only root, Squid-only
  egress) are the compensating controls.
  For the owner: [fallback] Codex's own sandbox is disabled; enabling it needs seccomp and AppArmor unconfined.
- **Repository configuration isolation.** Codex runs with cwd = the job dir and the checkout at `<job>/repo`, so
  the repository's AGENTS.md, `.agents/skills` and `.codex/` are not discovered; `project_doc_max_bytes = 0` and
  `features.hooks = false` as a second layer; the project is never marked trusted. The prompt names repository
  files (AGENTS.md and CLAUDE.md included) untrusted data (S7 finding 8).
- **Sub-agents off** (`features.multi_agent = false`). Child threads write their own rollouts, their usage is not
  in the parent's totals and `spawn_agent` is not visible in JSONL; S2 shows delegation fires unprompted. The
  prompt does not offer sub-agents.
  For the owner: [deviation] brief §5 and the owner decision allow sub-agents at the agent's discretion; v1
  disables them. Revisit when usage accounting and archives follow child threads.
- **Skills.** The runner owns a checkout of `lklimek/claudius` at a configured ref and exposes a read-only
  allow-list of skill directories at `$HOME/.agents/skills` of the agent uid; v1 list: `severity`.
  `[skills.bundled] enabled = false`. Prompts name the skill to apply. The resolved claudius commit is returned
  in `JobResult.skills_commit` and recorded per analysis. `report-format` is not mounted (it competes with the
  output schema).
- **Severity output.** The agent rates with `severity` but emits only the band `low | medium | high`; the
  skill's numeric likelihood/impact/relevance values never appear in any schema (brief §1).

## Consequences
- The runner image and WP3-OPS need no seccomp or AppArmor relaxation.
- The agent can read its per-job `CODEX_HOME` (credential copy, config); accepted under T1/T9 (WP1-T).

## Amendment (2026-11-22, SK1, ADR-023; superseded by the amendment of 2026-11-23)
- `verify_comments` jobs mount `severity` and `check-pr-comments` (`contracts::agent::verify_comments::SKILLS`);
  analysis and Ask jobs keep `severity`. The server hands a `verify_comments` job only those skills and the runner
  refuses one with any other (`misfit`); still no MemCan endpoint and no session. `report-format` stays unmounted.
  The prompt names both skills and states that the output schema and "no GitHub actions" win over any skill step
  (no gh or other scripts, no posting, no report.json or other file). Default `SR_RUNNER_SKILLS`:
  `severity,check-pr-comments`.
- The claudius commit is recorded per comment assessment (`CommentAssessment.skills_commit`) and shown as
  provenance ("Assessed with claudius <short sha>"). Owner Q28 = c: it is not part of the assessment fingerprint, so
  a new claudius commit never re-assesses anything; only new or changed items and "Assess again" use the current
  skills. The prompt text change is recorded under the same `VERIFY_PROMPT_VERSION` for the same reason.

## Amendment (2026-11-23, owner decision, comparison report R8 and R13)
- `verify_comments` jobs mount no skill (`JobKind::VerifyComments.required_skills()` is empty) and the runner refuses
  one that carries any (`misfit`). Their prompt states, in diffpilot's words, what they took from the skills: the four
  gates the output schema allows (G-FUNDS, G-SECRET, G-CRYPTO, G-DATA), the category catalogue, the necessity rule
  and the checking rules and decision kinds of `check-pr-comments`. Reading both skills cost about 37 KB of tool
  output per job (S13-3 §4) and their fetch, reply and report steps conflicted with the job.
- Analysis, repair and Ask jobs keep `severity`; the runner's claudius checkout, `SR_RUNNER_SKILLS` (default
  `severity`) and the required-skills check at start stay for them. A list that still names `check-pr-comments` is
  accepted.
- Comment assessments made from then on carry no claudius commit (`skills_commit` null).
