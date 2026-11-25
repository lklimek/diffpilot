# Threat model — diffpilot v1.3 containers

Status: current implementation model, 2026-11-24. [ADR-025](../decisions/ADR-025-container-runtime.md),
[CCR1–CCR13](containers.md) and SG1–SG16 replace the exec/supervisor credential-copy design. Scope: public
repositories, browser/server, runner/launcher, workers/fetch, gateway, Squid, GitHub and MemCan. This document
records controls and acceptance work; it does not attest an independent security review.

## 1. Boundaries and assets

| ID | Boundary / asset | Protection and residual exposure |
|---|---|---|
| B1 | PR code, comments, related work and MemCan → model | Untrusted data; SG16 blocks automatic checkout configuration. Persuasive injection remains possible. |
| B2 | Worker → host / other workers | Fixed non-root read-only container, cap-drop ALL, no-new-privileges, private per-payer work volume; no Docker socket or host bind. Kernel/container escape remains infrastructure risk. |
| B3 | Runner → launcher | Private 0600 unix socket; derived identifiers and fixed templates. Launcher has engine access and is trusted; never parses model output. |
| B4 | CS1/CS3 → network | Internal isolated bridge; Squid only, private destinations denied except configured MemCan host/port/address. Public exfiltration remains AR-3. |
| B5 | CS2 → network | Separate internal judge bridge, only gateway model listener; no Squid. No worker TCP listeners; app-server uses a unix socket. |
| B6 | Worker → gateway | Expiring hashed virtual model keys; model listener only. Admin binds a different interface, with distinct credentials/keys bearer scopes. |
| B7 | Gateway → provider | Dedicated encrypted refresh chains, own encryption key, fixed auth/backend upstreams; one owner/marker and persist-before-use. Unknown result quarantines. |
| B8 | Browser → server | Allow-listed numeric GitHub identity, session/CSRF, separate runner listener, ownership checks, text rendering and CSP. |
| B9 | Server → GitHub | User write credentials and App key server-only; job tokens read-only, PR-repository-scoped and revoked. |
| B10 | Assessment adapters → cards/effects | Frozen shared input; independent methods and attempt fences; grounded source/identity validation; only frozen primary writes visible state. |

| ID | Asset | Location / worker access |
|---|---|---|
| A1 | GitHub user tokens, App key, app encryption key | Server DB/files only; never workers. |
| A2 | Provider access/refresh tokens | Encrypted gateway store only; never runner/worker or app DB. |
| A3 | Virtual model key, job GitHub token | Worker per-job tmpfs files; readable by that worker, bounded lifetime. |
| A4 | Credentials/keys admin bearers | Gateway + server or runner respectively; never workers. Credentials file locally 10010:host-gid 0440, other gateway files 0400; runner copy 10002 0400. |
| A5 | Full MemCan bearer | CS1 runtime config in tmpfs; full cross-project permissions accepted (AR-16). |
| A6 | Thread history, checkout, normalized comparisons/snapshots | Per-(PR,payer) work volume and server DB. Same-payer Ask forks; others receive their private context pack. No host credential home. |
| A7 | Logs and browser output | Scrub job tokens/model keys; no proxy paths, queries or headers; text-only untrusted content. |

## 2. Runtime security rules

| ID | Rule |
|---|---|
| SG1 | User GitHub tokens and App key stay server-only. |
| SG2 | PR-repository read-only installation token, per job, ≤1 h, tmpfs file. |
| SG3 | Real provider credentials stay gateway-only; virtual keys ≤90 min, worker networks only. |
| SG4 | Workers have no engine socket, host paths, privileged mode or added capabilities. |
| SG5 | CS1/CS3 egress only through Squid; deny LAN except configured MemCan. CS2 reaches model gateway only. |
| SG6 | Full MemCan bearer in CS1 is accepted MC1/AR-16. |
| SG7 | Gateway admin, runner listener and launcher socket are unreachable from workers. |
| SG8 | The container replaces the uid/supervisor boundary. |
| SG9 | No exec credential-copy/environment/MCP-GitHub/recovery contract survives. |
| SG10 | Keep secret scrub, lease/attempt fencing, turn cap, payer usage pause and disk reserve. |
| SG11 | Sub-agents disabled. |
| SG12 | Render untrusted text as text; build links server-side. |
| SG13 | Parse reports with scope, quote, identity and duplicate checks. |
| SG14 | Lean config, hosted web search off, no worker toolchain/builds, strict schemas; judge tools fail. |
| SG15 | Gateway requires store:false, stream:true, no previous_response_id and recursively allowed tools. |
| SG16 | CS1 cwd /work; /work and /work/repo untrusted in both layers; critical overrides and full fixed kind config on start/fork/resume; no repo-skill discovery. |

## 3. Container threats and controls

| ID | Threat | Control / acceptance |
|---|---|---|
| T4 | Worker escape, resource exhaustion or PR-code execution | Fixed non-root container, cap-drop ALL, no-new-privileges, default seccomp, read-only root; configured memory/swap/CPU/PIDs, no toolchain. No builds/tests/PR-code execution is also a prompt rule. CS3 only fetches using sanitized manifests/config. Offline image and CS3-through-Squid tests. |
| T5 | Repository instructions/config/plugins alter runtime | SG16 system + rewritten user untrusted presets, fixed cwd, critical CLI overrides, repo-skill suppression, immutable kind config; worker reset and app-server config tests. Deliberately reading source remains possible. |
| T9 | Exfiltration through public egress or hosted tools | Squid logs destination host/port/bytes only; hosted web/image tools refused at the gateway. Public egress remains allow-all (AR-3). |
| T10 | Secrets in events, errors, output or histories | Exact-value and shape scrub, including job installation token, MemCan bearer and virtual model key; output/result caps; fake canary e2e leak tests. Gateway logs no credentials/auth bodies. |
| T34 | Launcher confused deputy / stale workers | Fixed templates, derived names/labels, private socket, no model parsing, startup reconciliation and local key revocation. No network on launcher. Launcher/image tests. |
| T35 | Gateway key theft, cross-scope admin calls or refresh replay | Hashed keys ≤90 min, invalid key 401 vs unlinked 403, listener binding, distinct admin scopes, path allowlist and SG15 body checks. Dedicated chain, single refresh owner, durable marker, persist-before-use; unknown outcome quarantines. Only explicit unlink revokes upstream. All auth tests fake-only. |
| T36 | Private-network or metadata access, DNS rebinding | Isolated internal worker/judge bridges; Squid denies resolved private addresses except pinned configured MemCan. `deploy/dev/test-local-containers.sh` probes actual launcher-created CS1/CS2 against live fake listeners (SG5/SG7), including direct/proxied admin, runner, metadata, direct Internet and MemCan. |
| T37 | Invented report identities, quotes or cross-source duplicates | Validate host schema/mode/PR/head, scope and verbatim grounded quote, source kind/ID reuse, direct canonical duplicates and primary-only effects. Host-report ingestion and comparison tests. |
| T38 | Job keys/tokens persist or enter environments | Tmpfs-only files, reset per job; local revoke on end/cancel/retirement; no credential in worker env or persistent volume. Worker reset and provider tests. |
| T39 | Checkout configuration survives warm reuse | Rewrite shape/config and remove planted instructions/skills before each start/fork/resume; /work and /work/repo untrusted in both layers; fixed cwd /work. Worker image/reset tests. |

## 4. Preserved application controls

| ID | Control | Owner | Verification |
|---|---|---|---|
| T3a | Mint body always has explicit `permissions = {contents, issues, metadata, pull_requests: read}` and explicit `repositories = [PR repository]` only, verified `private: false`; related repositories are not in the token (read as public data, S4 §1); response permissions/selection compared, mismatch → reject + revoke. PR repository in no installation → no token and no `gh` (`AgentAccess.github = None`): analysis runs (ingest with the user token), related work shown as "not checked", commenting disabled; never a token from another installation | WP3-APPTOKEN, WP3-RUN, WP3-PIPE | `installation_token_read_only_permissions` (asserts the request body), `installation_token_repositories_pr_repo_only`, `mint_rejects_private_repo`, `mint_response_with_write_permission_rejected`, `uninstalled_repo_job_has_no_github_access` (no mint call, config without `github`) |
| T3b | Token minted at claim time; never in persisted `jobs.spec`, events, ready outputs or results, never plaintext at rest; the only stored copy is AES-GCM ciphertext in `installation_tokens` (one row per job + attempt, ADR-005), kept solely to revoke it: read non-destructively, listed after a server restart to recover pending revocations, deleted only once revocation is confirmed; revoked (`DELETE /installation/token`) at every job end (success, failure, cancel, lease expiry — at lease expiry cancel wins: token revoked, ready output discarded); job runtime cap 45 min < 1 h lifetime | WP3-APPTOKEN, WP2-JOBS | `installation_token_revoked_on_job_end` (4 outcomes); `jobs_spec_has_no_credentials`, `installation_token_stored_only_encrypted_until_revoke_confirmed`, `pending_revocations_recovered_after_restart`, `cancel_at_lease_expiry_revokes_and_discards_output` |
| T3d | User tokens, App key, client secret only in `server::{auth,github,secrets}`; never in `JobSpec`, claim response, events, logs | WP2-AUTH, WP3-APPTOKEN | `check-architecture.sh` grep; serialisation test of `JobSpec` + claim response; log capture |
| T7a | Auth: cookie Secure (from `SR_PUBLIC_URL` scheme), HttpOnly, SameSite; session ids stored hashed; CSRF header on every cookie-authenticated mutating route; OAuth `state` bound to a pre-auth cookie, single use, short TTL, PKCE S256, exact `redirect_uri`, App wildcard callback matching off; allowlist checked before a session exists and bound to the numeric GitHub user id at the first successful login (later logins must match the id); refresh serialised per user, rotated pair stored before use; AES-GCM random nonce, AAD = table + column + row id; `secrecy` types | WP2-AUTH, WP3-AUTH | `allowlist_rejects_unlisted_login`, `allowlist_binds_github_user_id` (same login, other id → rejected), `secure_cookie_behind_trusted_proxy`, `oauth_state_single_use`, `csrf_required_on_mutations` (sweep of cookie-authenticated routes), `session_id_stored_hashed`, `token_ciphertext_bound_to_row` |
| T7b | Server surface: `/runner/*` only on a separate listener `SR_RUNNER_LISTEN_ADDR`, attached only to the internal Docker network; the public listener never serves `/runner/*`; runner routes use no cookie and no CSRF: `claim` needs the runner secret; `events`, `output` and `result` need that job's token (bound to job id + attempt, rejected for a stale attempt, after lease expiry or for another job), compared in constant time; body size caps on all three; public port published only to the reverse proxy (local stack: `127.0.0.1`); `X-Forwarded-*` only from `SR_TRUSTED_PROXIES`; headers: CSP `default-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'`, `nosniff`, `Referrer-Policy: same-origin`; `ApiError.message` never internal text; tenant check on every analysis route; `lines`/`diff`/agent paths resolved only in the selected pinned tree — base or head SHA of the analysis per `side` (`side=base` is legal for deleted code), no filesystem join — ranges capped; PR URL parsed to owner/repo/number (strict charset), no user-supplied URL fetched; SSE needs a session | WP2-JOBS, WP2-INGEST, WP3-STATE, WP3-OPS | `public_listener_rejects_runner_routes`, `runner_routes_require_secret_or_own_job_token` (claim, events, output, result), `output_rejected_for_stale_attempt`, `security_headers_present`, `forwarded_headers_ignored_from_untrusted_peer`, `lines_rejects_path_traversal` (both sides), `lines_side_base_serves_deleted_code`, `sse_requires_session`, `no_secret_in_logs` (log capture); compose test: runner network reaches only proxy and runner listener |
| T11 | Pending review: user token server-side only; only `addPullRequestReviewThread`/`updatePullRequestReviewComment`/`deletePullRequestReviewComment`; never submit; writes serialised per (user, PR); head check before each write; `thread: null` = error; idempotency key | WP3-REVIEW | WP3-REVIEW tests; `check-architecture.sh` greps for `submitPullRequestReview` and REST review `event` |
| T12 | Output integrity and rendering: schema + verify (for the ready output exactly as for the result); agent paths checked in the pinned base or head tree of their side; LLM, repository and GitHub text rendered as text only (no `dangerouslySetInnerHTML`; Shiki output from escaped tokens); no clickable link in LLM text except server-built GitHub links; CSP (T7b); size caps on events, ready outputs and results; flow files bound ids, heading words, field names and the section count (`agent-output-format.md`), so checking one needs memory proportional to its size | WP3-VERIFY, WP3-CODEVIEW, WP3-BLOCKS, WP3-INSPECTOR, WP2-JOBS | Playwright `agent_output_xss_renders_as_text` (fixture with `<img onerror>`, `javascript:`); `verify_rejects_path_outside_pinned_tree`; `result_and_output_size_capped`; `ids_heading_words_field_names_and_sections_are_bounded_with_line_numbers`, `diagnostic_line_map_stays_linear_in_the_document_size` |
| T15 | Usage pauses are per payer and reset times are bounded before scheduling; unknown reset uses the configured fallback pause. Turn deltas include compaction and exclude inherited fork usage; leases and attempt fencing remain mandatory. | runner/server | usage-limit and app-server accounting tests |
| T14 | Deterministic fact in PR context when the PR touches agent-instruction files (`AGENTS.md`, `CLAUDE.md`, `.agents/`, `.codex/`, `.claude/`, `.cursor*`, `.github/copilot-instructions.md`, …); server-set, not LLM content (contract field: CCR) | WP3-PREPASS, WP3-BLOCKS | `prepass_flags_agent_instruction_files`; PR-context component test |

## 5. Accepted risks

| ID | Risk and bound | Revisit trigger |
|---|---|---|
| AR-2 | Persistent worker history is agent-writable; only its (PR,payer) jobs and same-payer forks resume it. | Sharing worker state across payers. |
| AR-3 | Public egress through Squid can exfiltrate the worker's readable data. LAN remains denied except MemCan. | Private repositories or stronger egress requirements; add host allowlist. |
| AR-5 | PR text can steer LLM prose; labels, evidence and complete code view preserve human judgement. | New automated effects. |
| AR-7 | Docker operators can inspect service env secrets; file variants reduce accidental exposure. | More operators; prefer read-only *_FILE. |
| AR-8 | Persistent work/cache volumes share Docker disk; disk reserve stops new jobs but is no running-job quota. | Capacity pressure; quotas. |
| AR-13 | Compromised CS1 can spend its payer's plan until key expiry (≤90 min) or stop, and use its job's read-only GitHub token until job end. Keys are unusable off worker networks. | External tenants or higher privilege. |
| AR-14 | Injected comments can invent/hide points in their own source; grounded quotes, source/author identity and visible unassessed sources limit scope. | Automatic assessment decisions. |
| AR-16 | Full MemCan token permits reading/writing/deleting every project's memories; model tool filters are not token scopes. | First user outside trusted organisation. |
| AR-17 | Undocumented Codex backend/OAuth client may change or interfere with other CLI sessions; probability and blast radius unknown. Linked plans and refresh chains depend on gateway-store secrecy. Owner accepted KP1/KL1-A with visible logout-risk disclosure; no session-isolation guarantee. | Provider changes or supported delegated plan access. |

## 6. Release gates

No user/App/provider credential in workers, JobSpec persistence, browser or logs. Only the launcher's container
has Docker access. CS1/CS3 cannot bypass Squid or reach admin/runner/launcher; CS2 reaches only model gateway.
Known tokens are scrubbed; model content cannot publish a review, decide an effect, escape ownership checks or
render HTML. Every surviving historical GitHub/browser control below still applies. Security review is the
coordinator's separate gate; live networking and fake-provider journeys complement it.

## 11. v1.1 — GitHub work dashboard and personal access tokens

Plan `docs/v1.1-dashboard-plan.md` §E.4; ADR-016 to ADR-020. New assets: per-user snapshots (titles, labels and
300-character excerpts of private repositories, OD11), stored LLM judgements, MemCan TODO text, personal access
token hashes, and the GitHub user-token pair kept without a live session while the daily schedule is on. New entry
points: R1–R10 (`http-api.md` "Dashboard (v1.1)"), `Authorization: Bearer srp_…` on `read` routes, the in-process
scheduler, `judge_work` jobs. The v1.1 accepted risks are AR-9 and AR-10 (AR-7 above covers service environment secrets).

| # | Control (testable) | Owner | Verified by |
|---|---|---|---|
| T16 | Dashboard data per user: every repository function and query keyed by `(tenant_id, user_id)` of the session or token principal (no list-all API for serving); another user's refresh id is `not_found`; SSE streams per user; settings per user | WP11-DB, WP11-REFRESH, WP11-SCOPE | `dashboard_of_another_user_is_never_served`, `dashboard_events_are_per_user` |
| T17 | JudgeWork runs in ephemeral CS2 with filesystem-only runner setup, no checkout/GitHub/MemCan, gateway-only networking and fixed read-only thread config. Any model tool item fails the attempt; output sets, batch ids and text bounds are revalidated. | runner/server | runner app-server tests; live `test-local-containers.sh`; dashboard Playwright |
| T18 | The scan is read-only: `server::github::work` sends only REST `GET` and GraphQL queries (never a mutation), with the user token, which is never logged | WP11-GH | `work_client_sends_no_mutation`, log-capture test |
| T19 | Private-repository text is at rest in snapshots and judgements and sent to the provider in judge prompts (`jobs.spec`); retention bounded (10 refreshes per user plus the latest snapshot's, judgements 90 days after last seen, judge jobs by v1 job retention); TODOs read and shown only for users with `include_todos` | WP11-DB, WP11-REFRESH | retention tests; `todos_hidden_when_switched_off` |
| T20 | Personal access tokens (ADR-019): 256-bit random secret, only SHA-256 at rest, shown once (`CreatedAccessToken` is the only type carrying it, contract test), constant-time comparison, expiry 30/90/365 days, revocation (also by "sign out everywhere"), allow-list checked per request, 120 requests/min per token and 30 failed attempts/min per client (malformed tokens refused before any lookup; a well-formed one is looked up by its public id and compared in constant time, and a failure past the limit answers 429; a valid token is never refused by it), at most 16 open event streams per user (cookie or token), one audit line per request without the secret, its hash or query strings; accepted only on `read` routes (`GET`) of the public listener; never with a session cookie in the same request; never on the runner listener | WP11-TOKENS | `token_never_accepted_on_mutating_routes`, `token_cannot_list_or_mint_tokens`, `revoked_expired_or_unlisted_user_token_is_rejected`, `token_and_cookie_together_are_rejected`, `token_not_logged`, `runner_listener_rejects_personal_tokens`; contracts `token_secret_is_only_in_the_creation_response`, `tokens_are_accepted_only_on_public_safe_reads` |
| T21 | The GitHub token pair is kept without a live session while the schedule is on (time zone known) and the user signed in within the last 30 days (`SCHEDULE_PAIR_KEEP_DAYS`; ADR-020 addendum): AES-GCM at rest as in v1, the allow-list checked before each scheduled run, deleted by "sign out everywhere", allow-list removal, switching the schedule off, 30 days without a sign-in (the per-minute sweep), a rejected refresh or an expired refresh token | WP11-SCHED | `pair_kept_without_session_while_schedule_on`, `pair_kept_by_schedule_lapses_after_thirty_days_without_sign_in`, `sign_out_everywhere_deletes_pair_and_stops_schedule`, `unlisted_user_is_not_scheduled` |

| # | Risk | Blast radius | Why accepted | Cheapest later mitigation |
|---|---|---|---|---|
| AR-9 (T17) | Proposed: PR, issue and TODO text can skew the LLM score parts and texts within the closed value sets of the items judged in the same batch (up to 8: its own and its neighbours'), only for the parts the scorer asks about (including estimated defaults sent for confirmation); parts computed from facts are never set by the judge (an answer for one is dropped, ADR-017) | the ranks of the batch's items among the user's own work; never an action, a tool call or a write anywhere | the ranking is advisory; every part shows its source and basis; parts computed from facts stay deterministic (test `an_override_of_a_fact_computed_part_changes_nothing`) | per-part caps on LLM parts; compare with the computed estimates and mark large gaps |
| AR-10 (T20) | Accepted (owner, OD17): a token is as powerful as a read-only session of its user (dashboard, Tour, Q&A, pending comments, private-repository titles) and lives in the user's agents' environments and files outside the browser | everything its user can read in diffpilot until it expires or is revoked | the owner's agents already hold his GitHub access; tokens are read-only, expire, can be revoked, are rate limited and audited | narrower scopes per route group (`TokenScope` is open); shorter default expiry |

## 12. v1.3 — review loop (PR comments, decision feed, Watch, notifications)

Plan `docs/review-workflow-plan.md` §7; ADR-023. New assets: decisions and the owner's comments, LLM assessments of
review comments, carry-over evidence, agent claims and reports, watched-PR probes, notification channel and agent
push-target secrets (AES-GCM ciphertext in `notification_channels.secret` and `agent_subscriptions.push_secret`,
migration 0017). New entry points: RC1–RC11, RW1–RW3, RN1–RN3, RF1–RF5 (`http-api.md` "Review loop (v1.3)"), the
`review_loop_execute` token scope, outbound ntfy/webhook deliveries, `verify_comments` jobs.

| # | Control (testable) | Owner | Verified by |
|---|---|---|---|
| T22 | Decisions, gate marks, "Resolve this thread" and PR-level questions only with session + CSRF of an allow-listed user; every decision route is `session_csrf` (contract) and answers a token with 403 `token_not_accepted` | WP13-A, WP13-CORE | contracts `execute_scope_is_only_on_feed_write_routes`; `no_token_reaches_decision_routes`, `gate_resolve_needs_explicit_choice` |
| T23 | Verify uses a tmpfs read-only installation token, no MemCan, separate ephemeral method threads and base convention files as data. Native v4 strict schema and host report 4.2 are independently validated. Primary-only effects; posted drafts pass the assessor-wording guard at ingest/read and owner preview. | runner/server | assessment/host-report tests; review-loop Playwright |
| T24 | Comment bodies are untrusted (G-PHISH): plain text, links only `https://github.com/…`; markers parse known keys of `thepastaclaw-*` markers only (spike S13-1 M6; fixture thread `l6` carries unknown keys) | WP13-GH, WP13-WEB | `markers_parse_only_known_keys`, Playwright `comment_texts_render_as_text` |
| T25 | Execute tokens: `RouteAuth::TokenOnly(ReviewLoopExecute)` on RF3–RF5 only; claim and report only; reports only by the claimer under a live lease (`db::repo::review_loop` refuses others); idempotent reports; audit line and rate limit as ADR-019 | WP13-A, WP13-DB, WP13-FEED | contracts `execute_scope_is_only_on_feed_write_routes`; db `claims_hold_a_lease_and_reports_come_from_the_claimer`; `report_on_foreign_claim_is_rejected`, `execute_token_cannot_decide` |
| T26 (phase 3) | Write installation token: server-only, PR repository, per effect, revoked; only effects of stored decisions | phase 3 | `write_token_never_in_job_spec`, `bot_posts_only_decided_effects` |
| T27 | Watch probes are reads with the watcher's own token; the token pair is kept only within ADR-020's 30-day bound; watches pause without it | WP13-WATCH | `watch_probe_sends_no_mutation`, `watch_pauses_without_token_pair` |
| T28 | Channel and push-target secrets: AES-GCM at rest (the database holds ciphertext only; record `Debug` redacts it), masked in every response (`secret_last4`), never logged, never in a `JobSpec`; push payloads carry ids and links only | WP13-A, WP13-DB, WP13-NOTIFY | contracts `channel_responses_carry_no_secret_field`, `push_payloads_have_a_fixed_shape`; db `channel_secrets_stay_ciphertext_and_health_changes_once`; `channel_secret_is_masked_in_responses`, `job_spec_has_no_channel_secret` |
| T29 | Outbound notifications: http/https only (422 `scheme_not_allowed`), no redirects, 10 s timeout, response read ≤ 4 KiB, per-channel rate counted from stored sends, loopback/link-local/reserved addresses always refused (URL and resolution, no proxy), optional private-address deny (`SR_NOTIFY_DENY_PRIVATE=1`), a kept secret never follows a changed kind or origin, pushes only to live tokens of allowed users | WP13-NOTIFY | `notify_rejects_non_http_scheme`, `notify_does_not_follow_redirects`, `notify_denies_private_when_configured`, `notify_refuses_loopback_and_link_local_even_when_private_allowed`, `channel_limit_survives_delete_and_recreate`, `url_change_with_kept_secret_is_refused`, `kind_change_with_kept_secret_is_refused`, `subscriptions_of_users_off_the_allow_list_are_absent`, `subscriber_push_targets_of_other_users_are_hidden` |

| # | Risk | Blast radius | Why accepted | Cheapest later mitigation |
|---|---|---|---|---|
| AR-11 (T23) | Proposed: a prompt-injected comment can skew its own LLM assessment and recommendation within the closed sets | the recommendation shown for that item; never an action: every decision is the owner's, with the evidence shown | the owner decides each item (gate items never in bulk); the assessment is tagged LLM | flag recommendations that contradict the bot's severity or the evidence lines |

## 13. v1.4 webhook, imported decisions and activity

| ID | Threat | Contract and acceptance |
|---|---|---|
| T40 | Forged, replayed or oversized GitHub deliveries, proxy routes exposing the UI, and payload leakage | Route only `POST /webhooks/github` to the separate webhook listener. The WAF is not authentication. GitHub signature verification over raw bytes and App target checks authenticate deliveries. Processing is limited to repositories tracked by a tenant, matched by numeric GitHub repository id when supplied and by owner/name otherwise. The receiver bounds size and time before parsing and stores typed changes and error codes. Delivery-id and raw-body hash dedupe use a 14-day retention window. A valid captured body can be replayed beyond the window, since the signature covers the body and the delivery id is unsigned. Treat accepted events as signals to re-read GitHub. Traefik and cloudflared must keep request bodies out of logs. B1 verifies listener isolation. Traefik requires version 3.6.4 or newer, or 2.11.32 on the 2.x branch. Receiver checks follow [ADR-026](../decisions/ADR-026-github-webhooks.md). A LAN peer that can reach port 3002 can occupy all 64 connection slots and cause missed deliveries, with facts recovered by Refresh or the nightly scan. |
| T41 | Untrusted thread text is mistaken for an authorised decision | Ground the quote and source, verify authority and topic, and require a deterministic cue. Match the comment author's numeric GitHub id against the member's stored id. A missing id carries no authority. A member's linked login can relay only that member's decision with the structured marker and a matching resolved GitHub id. A linked login shared by multiple members has no relay authority. Phase B rule I6 prohibits effects for imported decisions, including replies, resolves and agent work. Author withdrawals apply only to the author's own items and cannot decide other authors' followers. See [import rules](../v1.4-phase-b-plan.md#7-imported-github-decisions-c11). |
| T42 | LLM activity exposes another user's subjects, times or usage pauses | Scope reads to the reader's tenant and project each response for the reader on HTTP and SSE. A peer job has kind and a response-local opaque id. Real job ids, subjects, times, wait reasons and payer pause or call outcome are private to the payer. Repository coverage is scoped to the reader's tenant's tracked repositories. Webhook status exposes an installation-wide count of drops without repository or account details. See [activity contract](../v1.4-phase-b-plan.md#8-llm-activity-c7). |
