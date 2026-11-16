# ADR-002: Codex runtime — model, per-job configuration, sandbox, repository isolation, sub-agents, skills

- Status: accepted (coordinator, 2026-11-16, after spikes S1a, S1b, S2, S7)
- Source: `docs/spikes/S1a-codex-basics.md`, `S1b-codex-sessions-isolation.md`, `S2-claudius-skills.md`,
  `S7-llm-capability.md`; plan §5.2–§5.4; brief §5.

## Decisions
- **Model.** "Sol" is `gpt-6.1-sol`. `select_model` returns `gpt-6.1-sol` with reasoning effort `high` for every
  job kind. The runner passes the id as `model` and the effort as `model_reasoning_effort` in the per-job config.
  `select_model` is the only place to change either. Resolves the *pending spike S1a* items of ADR-001.
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
