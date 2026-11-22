// The review loop as the mocks' server holds it (docs/review-workflow-plan.md, mockup docs/mockups/10-review-loop.html):
// the PR page, items with assessments and decisions, the log, rounds, carry-overs, watches and notification
// channels of the fixture PR acme/node-client #482. Decisions change the store until `resetReviewLoop`.
// Mockup states are selected with scenarios (`comments` of scenarios.ts).
// FIXTURE (contract gaps, see TODO(WP13-A CCR)): Ask answer counts per comment item use the pending `comment_item`
// anchor of api/pending.ts; there is no Undo or Reopen route, no daily auto-pass count and no PR author on PrPage.
import type {
  AnalysisSummary,
  CarryEvent,
  CarryOverPage,
  CodeContext,
  CodeLine,
  CommentAssessment,
  CommentItem,
  CommentLog,
  CommentsOverview,
  DecideRequest,
  Decision,
  DecisionKind,
  Effect,
  Gate,
  NotificationSettings,
  PrBadgeView,
  PrPage,
  PrRef,
  QueueEntry,
  Recommendation,
  WatchedPr,
  WatchList,
} from "../api/generated/types";
import { plannedEffects, type Choice } from "../components/comments/decisions";
import { liveAnalysis } from "./live-analysis";
import { scenario } from "./scenarios";

/** Mockup states of the review loop (scenario field `comments`). */
export type CommentsMockState = "no-analysis" | "running" | "empty" | "github-error" | "watched";

const state = (): CommentsMockState | undefined => scenario().comments;

/** The fixture PR. */
export const PR: PrRef = { repo: { owner: "acme", name: "node-client" }, number: 482 };
const HEAD = "52d2da140191a9d47becf6a446310be803c007fc";
const OLD_HEAD = "3f9a1c2e4b5d6f708192a3b4c5d6e7f809a1b2c3";
const URL_PR = "https://github.com/acme/node-client/pull/482";
const at = (hhmm: string, day = "18") => `2026-11-${day}T${hhmm}:00Z`;
const READ_AT = at("09:20");

const rec = (
  kind: DecisionKind,
  reason: string,
  reason_class: string,
  target: Recommendation["target"] = null,
): Recommendation => ({ kind, reason, reason_class, target });

/** An assessment at the head with the given fields. */
function assessment(fields: Partial<CommentAssessment> & Pick<CommentAssessment, "recommendation">) {
  return {
    job: "7c9e6679-7425-40de-944b-e07fc1f90ae7",
    head: HEAD,
    assessed_at: at("09:12"),
    reused_since: null,
    validity: "valid",
    evidence: [],
    pre_existing: false,
    category: "correctness",
    necessity: { verdict: "in", reason: "The PR promises this behaviour.", better_pr: null },
    gates: [],
    elsewhere: [],
    fix_sketch: null,
    fix_size: null,
    reply_drafts: [],
    code: null,
    prompt_version: 1,
    ...fields,
  } satisfies CommentAssessment;
}

const facts = (fields: Partial<CommentItem["github"]> = {}): CommentItem["github"] => ({
  resolved: false,
  resolved_by: null,
  outdated: false,
  bot_status: null,
  edited_after_decision: false,
  replies: 0,
  marker: null,
  ...fields,
});

/** A thread item of thepastaclaw at the head. */
function item(no: number, fields: Partial<CommentItem> & Pick<CommentItem, "title" | "body">): CommentItem {
  return {
    id: `00000000-0000-4000-8000-${String(no).padStart(12, "0")}`,
    no,
    kind: "thread",
    stable_key: `thread-${no}`,
    author: "thepastaclaw",
    author_kind: "bot",
    url: `${URL_PR}#discussion_r10${no}`,
    location: { path: "src/rpc/client.rs", lines: { start_line: 90, end_line: 90 } },
    bot_severity: "Suggestion",
    gates: [],
    state: "undecided",
    revision: 1,
    first_seen_head: HEAD,
    first_round: 4,
    github: facts({ marker: `dedupe=finding-${no}` }),
    outside: null,
    repeat_of: null,
    assessment_state: { state: "assessed" },
    assessment: null,
    decisions: [],
    ask_answers: 0,
    ...fields,
  };
}

const gate = (g: Gate) => [{ gate: g, source: "assessment" as const }];

/** An effect of a stored decision. */
function effect(
  decision: string,
  seq: number,
  fields: Partial<Effect> & Pick<Effect, "kind" | "state">,
): Effect {
  return {
    id: `${decision.slice(0, 24)}${String(seq).padStart(12, "0")}`,
    seq,
    after: seq > 1 ? seq - 1 : null,
    executor: "agent",
    params: { text: null, thread_id: null, review_id: null, title: null, url: null, item_nos: [] },
    claim: null,
    reports: [],
    seen_on_github_at: null,
    updated_at: at("09:00"),
    ...fields,
  };
}

/** A stored decision. */
function decision(no: number, fields: Partial<Decision> & Pick<Decision, "kind" | "decided_at">): Decision {
  const id = `d0000000-0000-4000-8000-${String(no).padStart(12, "0")}`;
  return {
    id,
    item_no: no,
    reason: null,
    target: null,
    override: null,
    resolve_ticked: true,
    reply_text: null,
    comment: null,
    recommendation: null,
    via: "question",
    carried_from: null,
    supersedes: null,
    head: HEAD,
    decided_by: "alice",
    effects: [],
    ...fields,
  };
}

const D8 = decision(8, {
  kind: "reject",
  override: { type: "blocker_gate", gate: "G-FUNDS" },
  reason: "Pre-existing fee display; tracked in #455.",
  reply_text:
    "Per maintainer decision, accepted for this pull request: Pre-existing fee display; tracked in #455.",
  comment: "Reply only; no code change in this PR.",
  recommendation: rec("reject", "Pre-existing on base.", "pre_existing"),
  decided_at: at("08:45"),
});
D8.effects = [
  effect(D8.id, 1, {
    kind: "reply",
    state: "done",
    reports: [
      {
        outcome: "done",
        reference: `${URL_PR}#discussion_r2008`,
        message: null,
        token_name: "laptop-cli",
        at: at("08:47"),
      },
    ],
    seen_on_github_at: at("08:48"),
  }),
  effect(D8.id, 2, { kind: "resolve", state: "done", seen_on_github_at: at("08:48") }),
];
const D5 = decision(5, {
  kind: "handled_in_pr",
  target: { type: "existing_pr", number: 490 },
  via: "table",
  decided_at: at("08:31"),
  recommendation: rec("handled_in_pr", "#490 changes the same constant.", "other_pr", {
    type: "existing_pr",
    number: 490,
  }),
});
D5.effects = [
  effect(D5.id, 1, { kind: "reply", state: "done" }),
  effect(D5.id, 2, { kind: "resolve", state: "done" }),
];
const D16 = decision(16, {
  kind: "reject",
  reason: "The retry is idempotent by design.",
  via: "carried",
  carried_from: "d0000000-0000-4000-8000-000000000003",
  decided_at: at("08:47"),
});
D16.effects = [
  effect(D16.id, 1, { kind: "reply", state: "done", seen_on_github_at: at("08:48") }),
  effect(D16.id, 2, { kind: "resolve", state: "done" }),
];
const D17 = decision(17, {
  kind: "handled_in_pr",
  target: { type: "existing_pr", number: 490 },
  via: "carried",
  carried_from: D5.id,
  decided_at: at("08:47"),
});
D17.effects = [
  effect(D17.id, 1, {
    kind: "reply",
    state: "claimed",
    claim: { token_name: "build-host-2", lease_until: at("09:51") },
  }),
  effect(D17.id, 2, { kind: "resolve", state: "waiting" }),
];

/** Lines 80–100 of src/rpc/client.rs at the head (fictional). */
const CODE: CodeLine[] = [
  "pub async fn send_with_retry(&self, req: SignedRequest) -> Result<Reply, RpcError> {",
  "    let mut delay = self.policy.initial;",
  "    for attempt in 0..self.policy.max_attempts {",
  "        match timeout(self.policy.per_try, self.send(&req)).await {",
  "            Ok(Ok(reply)) => return Ok(reply),",
  "            Ok(Err(e)) if !e.is_retryable() => return Err(e),",
  "            _ => {}",
  "        }",
  "        // the first send may still be in flight here",
  "        sleep(delay).await;",
  "        delay = (delay * 2).min(self.policy.max_delay);",
  '        tracing::debug!(attempt, "retrying signed request");',
  "    }",
  "    Err(RpcError::Exhausted)",
  "}",
].map((text, i) => ({ line: 82 + i, text }));

function seedItems(): CommentItem[] {
  return [
    item(24, {
      kind: "pr_question",
      stable_key: "pr:3:merge",
      author: "diffpilot",
      author_kind: "diffpilot",
      url: null,
      location: null,
      bot_severity: null,
      first_round: 3,
      github: facts({ resolved: null, outdated: null }),
      title: "Merge now, remaining items as follow-ups?",
      body: "Round 3 of thepastaclaw has ended. Open items: 5 undecided, none with a gate. Remaining: C11, C22, C23, C18, C19.",
    }),
    item(21, {
      state: "reopened",
      repeat_of: 3,
      title: "Retry cap is checked after the sleep",
      body: "The cap on max_attempts is checked after sleeping, so the last attempt waits for nothing.",
      location: { path: "src/rpc/client.rs", lines: { start_line: 90, end_line: 91 } },
      github: facts({ marker: "dedupe=retry-cap-after-sleep" }),
      assessment: assessment({
        category: "correctness",
        necessity: { verdict: "out", reason: "The cap is a soft limit by design.", better_pr: null },
        evidence: [{ path: "src/rpc/client.rs", line: 91, note: "sleep before the cap check" }],
        recommendation: rec("reject", "The cap is a soft limit by design.", "soft_limit"),
      }),
    }),
    item(12, {
      state: "asked_back",
      title: "unwrap_or_default hides a parse error",
      body: "The reply parser falls back to a default reply when the body does not parse.",
      location: { path: "src/rpc/client.rs", lines: { start_line: 85, end_line: 85 } },
      ask_answers: 1,
      assessment: assessment({
        pre_existing: true,
        necessity: { verdict: "out", reason: "Pre-existing on base; moved, not added.", better_pr: null },
        recommendation: rec("reject", "Pre-existing on base; this PR moves the line.", "pre_existing"),
      }),
    }),
    item(14, {
      gates: gate("G-FUNDS"),
      bot_severity: "Blocking",
      first_round: 3,
      first_seen_head: OLD_HEAD,
      title: "Retry may resend a signed request after a late reply",
      body: "When the reply to the first send arrives after the timeout, send_with_retry sends the same signed request again. Both can be accepted by the node. See https://github.com/acme/node-client/blob/main/docs/nonce.md and https://evil.example/phish.",
      location: { path: "src/rpc/client.rs", lines: { start_line: 90, end_line: 93 } },
      github: facts({ marker: "dedupe=rpc-retry-resend" }),
      assessment: assessment({
        validity: "partly_valid",
        category: "funds",
        gates: ["G-FUNDS"],
        evidence: [
          { path: "src/rpc/client.rs", line: 91, note: "sends again without looking at the first attempt" },
        ],
        necessity: { verdict: "in", reason: "The description promises one send per call.", better_pr: null },
        fix_sketch: "Keep an attempt id; skip the resend once the first reply lands.",
        fix_size: "S",
        recommendation: rec("fix_here", "Introduced here; the PR promises one send per call.", "introduced"),
        code: {
          path: "src/rpc/client.rs",
          enclosing: { start_line: 82, end_line: 96 },
          description:
            "Sends the signed request with a per-attempt timeout; on a timeout it sleeps, doubles the delay up to the cap and sends the same request again. A late reply to an earlier attempt is not looked at.",
        },
      }),
    }),
    item(15, {
      gates: gate("G-SECRET"),
      title: "Debug log prints the endpoint token",
      body: "The debug line formats the whole request, including the bearer token of the endpoint.",
      location: { path: "src/rpc/client.rs", lines: { start_line: 93, end_line: 93 } },
      assessment: assessment({
        category: "secrets",
        gates: ["G-SECRET"],
        recommendation: rec("fix_here", "Introduced here.", "introduced"),
      }),
    }),
    item(11, {
      title: "Backoff cap is not configurable",
      body: "max_delay is a constant in RetryPolicy::default(). Deployments with slow nodes cannot raise it without a rebuild.",
      location: { path: "src/config.rs", lines: { start_line: 20, end_line: 20 } },
      ask_answers: 1,
      github: facts({ marker: "dedupe=rpc-backoff-cap-config" }),
      assessment: assessment({
        category: "maintainability",
        necessity: {
          verdict: "out",
          reason: "The PR promises retries, not configurable timing.",
          better_pr: "acme/node-client#490",
        },
        elsewhere: [
          { kind: "open_pr", reference: "acme/node-client#490", note: "changes the same constant" },
        ],
        fix_sketch: "Add a builder method.",
        fix_size: "S",
        recommendation: rec("reject", "Not promised by this PR; #490 owns the cap.", "not_promised"),
      }),
    }),
    item(22, {
      title: "Jitter uses a fixed seed",
      body: "The jitter RNG is seeded with a constant, so every client backs off in lockstep.",
      assessment: assessment({
        fix_size: "XS",
        recommendation: rec("fix_here", "Small and in scope.", "small"),
      }),
    }),
    item(23, {
      kind: "review_body",
      author: "octo-reviewer",
      author_kind: "human",
      bot_severity: null,
      stable_key: "review-77:1",
      url: `${URL_PR}#pullrequestreview-77`,
      title: "Doc comment names the old default",
      body: "The doc comment of RetryPolicy still says 30 s.",
      location: { path: "src/config.rs", lines: { start_line: 12, end_line: 12 } },
      github: facts({ resolved: null, outdated: null }),
      assessment: assessment({ fix_size: "XS", recommendation: rec("fix_here", "Doc fix.", "small") }),
    }),
    item(20, {
      author: "coderabbitai[bot]",
      title: "Harden temp file permissions",
      body: "The cache file is created with the default umask.",
      location: { path: "src/lib.rs", lines: { start_line: 10, end_line: 12 } },
      assessment: assessment({
        pre_existing: true,
        recommendation: rec("follow_up_pr", "Pre-existing on base.", "pre_existing", {
          type: "follow_up_pr",
          title: "Harden temp file permissions",
        }),
      }),
    }),
    item(18, {
      title: "Config file could be swapped by a local user",
      body: "A local user could replace the config file between the check and the read.",
      location: { path: "src/config.rs", lines: { start_line: 14, end_line: 16 } },
      assessment: assessment({
        category: "security",
        recommendation: rec(
          "reject",
          "Outside this PR's threat model: it needs an attacker running as the same OS user.",
          "threat_model",
        ),
      }),
    }),
    item(19, {
      title: "Cache directory symlink race",
      body: "The cache directory may be a symlink planted by another process.",
      location: { path: "src/lib.rs", lines: { start_line: 8, end_line: 9 } },
      assessment: assessment({
        category: "security",
        recommendation: rec(
          "reject",
          "Outside this PR's threat model: it needs an attacker running as the same OS user.",
          "threat_model",
        ),
      }),
    }),
    item(16, {
      state: "done",
      repeat_of: 3,
      title: "Double click sends twice",
      body: "A double click may send the request twice.",
      decisions: [D16],
      github: facts({ resolved: true, resolved_by: "laptop-cli", marker: "dedupe=retry-double-click" }),
      assessment: assessment({ recommendation: rec("reject", "Idempotent by design.", "by_design") }),
    }),
    item(17, {
      state: "executing",
      repeat_of: 5,
      title: "Backoff cap should be 30 s",
      body: "The cap should be 30 s.",
      decisions: [D17],
      assessment: assessment({
        recommendation: rec("handled_in_pr", "#490.", "other_pr", { type: "existing_pr", number: 490 }),
      }),
    }),
    item(8, {
      state: "done",
      gates: gate("G-FUNDS"),
      first_round: 2,
      title: "Fee shown before the retry adds a second charge",
      body: "The fee display does not account for a retried call.",
      decisions: [D8],
      ask_answers: 2,
      github: facts({ resolved: true, resolved_by: "laptop-cli" }),
      assessment: assessment({
        pre_existing: true,
        gates: ["G-FUNDS"],
        recommendation: rec("reject", "Pre-existing on base.", "pre_existing"),
      }),
    }),
    item(5, {
      state: "done",
      first_round: 1,
      title: "Backoff cap should be 30 s",
      body: "Cap the backoff at 30 s.",
      decisions: [D5],
      github: facts({ resolved: true, resolved_by: "laptop-cli" }),
      assessment: assessment({
        recommendation: rec("handled_in_pr", "#490 changes the same constant.", "other_pr", {
          type: "existing_pr",
          number: 490,
        }),
      }),
    }),
    item(9, {
      state: "outside",
      first_round: 2,
      title: "Rename attempt to try_no",
      body: "Nit: rename attempt.",
      github: facts({ resolved: true, resolved_by: "octo-dev" }),
      outside: { by: "octo-dev", seen_at: at("08:10") },
    }),
  ];
}

let store: { items: CommentItem[]; decided: number } | null = null;
const items = (): CommentItem[] => {
  store ??= { items: seedItems(), decided: 0 };
  return store.items;
};

/** Back to the seeded items (screen tests). */
export function resetReviewLoop(): void {
  store = null;
}

const OPEN_STATES = new Set(["undecided", "reopened", "asked_back"]);
const isOpen = (i: CommentItem) => OPEN_STATES.has(i.state);

/** Items as RC4 answers them in the selected state. */
export function listItems(): CommentItem[] {
  const all = items();
  switch (state()) {
    case "empty":
      return all.filter((i) => !isOpen(i));
    case "running":
      return all.map((i) =>
        isOpen(i) && i.no % 2 === 0
          ? { ...i, state: "assessing", assessment_state: { state: "running" }, assessment: null }
          : i,
      );
    default:
      return all;
  }
}

/** Queue order (plan §2.4): PR-level, asked again, answered, gate, blocking, single, groups. */
function queue(list: CommentItem[]): QueueEntry[] {
  const open = list.filter(isOpen);
  const by = (reason: QueueEntry["reason"], pick: (i: CommentItem) => boolean) =>
    open.filter(pick).map((i) => ({ item_nos: [i.no], reason }));
  const taken = new Set<number>();
  const take = (entries: QueueEntry[]) =>
    entries.filter((e) => e.item_nos.every((n) => !taken.has(n) && taken.add(n)));
  const groupNos = open
    .filter((i) => i.assessment?.recommendation.reason_class === "threat_model" && i.gates.length === 0)
    .map((i) => i.no);
  return [
    ...take(by("pr_question", (i) => i.kind === "pr_question")),
    ...take(by("asked_again", (i) => i.state === "reopened")),
    ...take(by("answered", (i) => i.state === "asked_back")),
    ...take(by("gate", (i) => i.gates.length > 0)),
    ...take(by("blocking", (i) => i.bot_severity === "Blocking")),
    ...take(by("single", (i) => !groupNos.includes(i.no) || groupNos.length < 2)),
    ...(groupNos.length >= 2 ? take([{ item_nos: groupNos, reason: "group" }]) : []),
  ];
}

const READ = () => ({
  read_at: state() === "github-error" ? at("08:12") : READ_AT,
  threads: 14,
  complete: true,
  reviews: 4,
  comments: 3,
  failure: state() === "github-error" ? ("rate_limited" as const) : null,
  next_read_at: state() === "github-error" ? at("09:52") : at("09:21"),
});

function counts(list: CommentItem[]) {
  const effects = list.flatMap((i) => i.decisions.at(-1)?.effects ?? []);
  return {
    unresolved_threads: list.filter((i) => i.kind === "thread" && i.github.resolved === false).length,
    undecided: list.filter(isOpen).length,
    asked_again: list.filter((i) => i.state === "reopened").length,
    waiting_for_agents: list.filter((i) => i.state === "executing").length,
    failed_effects: effects.filter((e) => e.state === "failed").length,
    outside: list.filter((i) => i.state === "outside").length,
    total: list.length,
  };
}

function assessmentProgress(list: CommentItem[]) {
  const needs = list.filter((i) => i.kind !== "pr_question" && i.state !== "outside");
  const assessed = needs.filter((i) => i.assessment_state.state === "assessed").length;
  return {
    running: state() === "running",
    assessed,
    total: needs.length,
    limited: 0,
    head: HEAD,
    at: state() === "running" ? null : at("09:12"),
  };
}

function badge(list: CommentItem[]): PrBadgeView {
  const c = counts(list);
  if (c.undecided > 0)
    return {
      badge: "questions_waiting",
      facts: [`${c.undecided} undecided questions (GitHub read ${READ().read_at.slice(11, 16)} UTC)`],
      missing: [],
    };
  if (c.waiting_for_agents > 0)
    return {
      badge: "executing",
      facts: [`${c.waiting_for_agents} decisions with open effects`],
      missing: [],
    };
  return { badge: null, facts: [], missing: ["CI of the head not read"] };
}

/** Rounds of thepastaclaw (V4). */
const ROUNDS: CommentsOverview["rounds"] = [
  ["a1b2c3d", at("09:40", "16"), 6, 0, 0, 0, 0, { fix_here: 4, reject: 1, follow_up_pr: 1 }],
  ["c4d5e6f", at("15:12", "16"), 3, 2, 1, 0, 0, { fix_here: 2, reject: 2 }],
  [OLD_HEAD, at("11:05", "17"), 4, 1, 2, 1, 0, { fix_here: 1, reject: 2 }],
  [HEAD, at("08:20"), 5, 2, 3, 1, 9, {}],
].map(([head, finished_at, n, open, carried, again, undecided, kinds], i) => ({
  number: i + 1,
  url: `${URL_PR}#pullrequestreview-${100 + i}`,
  head: head as string,
  finished_at: finished_at as string,
  verdict: "CHANGES_REQUESTED",
  new: n as number,
  still_open: open as number,
  carried: carried as number,
  asked_again: again as number,
  undecided: undecided as number,
  decisions: Object.entries(kinds as Record<string, number>).map(([kind, count]) => ({
    kind: kind as DecisionKind,
    count,
  })),
}));

const PURPOSE =
  "Retries a timed-out RPC call with exponential backoff instead of failing at once; a call is still meant to be sent once.";

/** The latest analysis, or none (`no-analysis`). */
const analysis = (): AnalysisSummary | null => (state() === "no-analysis" ? null : liveAnalysis());

/** RC2. */
export function overview(): CommentsOverview {
  const list = listItems();
  return {
    pr: PR,
    title: "feat(rpc): retry with backoff on timeout",
    url: URL_PR,
    head: HEAD,
    purpose: PURPOSE,
    read: READ(),
    counts: counts(list),
    rounds: ROUNDS,
    assessment: assessmentProgress(list),
    queue: queue(list),
    badge: badge(list),
    analysis: analysis()?.id ?? null,
  };
}

let watched: WatchedPr | null = null;
/** The reader unwatched the fixture PR (it leaves the list). */
let unwatched = false;

const WATCH_482 = (): WatchedPr => ({
  pr: PR,
  title: "feat(rpc): retry with backoff on timeout",
  since: at("09:02", "17"),
  last_probe_at: at("09:25"),
  last_auto_pass: { at: at("09:12"), questions: 5 },
  state: "active",
  ended_at: null,
  badge: badge(listItems()),
});

/** RC1. */
export function prPage(): PrPage {
  const list = listItems();
  return {
    pr: PR,
    title: "feat(rpc): retry with backoff on timeout",
    url: URL_PR,
    state: "OPEN",
    draft: false,
    base: "main",
    stack_parent: null,
    head: HEAD,
    ci: {
      state: "pending",
      total: 3,
      failing: [],
      check_runs: { passed: 2, failed: 0, pending: 1, skipped: 0, neutral: 0 },
      statuses: { passed: 0, failed: 0, pending: 0, skipped: 0, neutral: 0 },
    },
    plugin_notes: [
      {
        plugin: "pr-hygiene",
        source: "PR Hygiene",
        url: "https://hygiene.example/acme/node-client/482",
        as_of: at("09:19"),
        summary: "Waiting on author",
        details: ["2 blockers"],
        facts: [{ label: "Blockers", value: "2" }],
      },
      {
        plugin: "thepastaclaw",
        source: "thepastaclaw status",
        url: "https://thepastaclaw.github.io/review-system/",
        as_of: at("09:01"),
        summary: "reviewing (verify, 3/5) since 08:52",
        details: [],
        facts: [],
      },
    ],
    watch: watched ?? (state() === "watched" && !unwatched ? WATCH_482() : null),
    badge: badge(list),
    analysis: analysis(),
    comments: {
      counts: counts(list),
      rounds: ROUNDS.length,
      assessment: assessmentProgress(list),
      read: READ(),
    },
  };
}

/** Watch or unwatch the fixture PR (RW1). */
export function setWatched(on: boolean): void {
  watched = on ? { ...WATCH_482(), since: new Date().toISOString(), last_auto_pass: null } : null;
  unwatched = !on;
}

/** Back to no watch (screen tests). */
export function resetWatch(): void {
  watched = null;
  unwatched = false;
}

/** RC5: lines around the item, flows and notes from the analysis (none without one). */
export function context(no: number, from: number | null, to: number | null): CodeContext | null {
  const it = items().find((i) => i.no === no);
  if (!it?.location) return null;
  const first = from ?? CODE[0]!.line;
  const last = to ?? CODE.at(-1)!.line;
  const lines = CODE.filter((l) => l.line >= first && l.line <= last);
  const withAnalysis = analysis();
  const base = withAnalysis ? `/analyses/${withAnalysis.id}` : null;
  const plugins = [
    {
      source: "pr_hygiene" as const,
      label: "PR Hygiene: waiting on author, 2 blockers. Opens PR Hygiene in a new tab.",
      count: 2,
      url: "https://hygiene.example/acme/node-client/482",
    },
    {
      source: "thepastaclaw" as const,
      label:
        "thepastaclaw status: reviewing (verify, 3/5), data of 09:01. Opens the status page in a new tab.",
      count: null,
      url: "https://thepastaclaw.github.io/review-system/",
    },
  ];
  return {
    path: it.location.path,
    head: HEAD,
    lines,
    total_lines: 180,
    marked: it.location.lines,
    code: it.assessment?.code ?? null,
    analysis: withAnalysis?.id ?? null,
    analysis_head: withAnalysis ? HEAD : null,
    in_flows: base
      ? [
          { flow: "f1", flow_title: "Send an RPC request with retry", step: "f1.2" },
          { flow: "f5", flow_title: "Move of backoff helpers util/backoff.rs → rpc/retry.rs", step: "f5.1" },
        ]
      : [],
    notes: base
      ? [
          {
            source: "flows",
            label: "Flows: the LLM flagged these lines (1 flag). Opens the step in a new tab.",
            count: 1,
            url: `${base}#f1.2`,
          },
          {
            source: "files",
            label: "Files: incidental-change note on these lines. Opens the file in a new tab.",
            count: null,
            url: `${base}/files`,
          },
          {
            source: "dependencies",
            label: "Dependencies: 3 changed callers. Opens the graph in a new tab.",
            count: 3,
            url: `${base}/deps`,
          },
          ...plugins,
        ]
      : plugins,
  };
}

/** A stored decision for `it` from a request (RC8, RC9). */
function store_decision(it: CommentItem, body: DecideRequest, via: Decision["via"]): Decision {
  store!.decided += 1;
  const id = `e0000000-0000-4000-8000-${String(store!.decided).padStart(12, "0")}`;
  const choice: Choice = body.override?.type === "blocker_gate" ? "accept" : body.kind;
  const planned = plannedEffects(choice, body.resolve_ticked, it.kind === "thread");
  const decided: Decision = {
    id,
    item_no: it.no,
    kind: body.kind,
    reason: body.reason,
    target: body.target,
    override: body.override,
    resolve_ticked: body.resolve_ticked,
    reply_text: body.reply_text,
    comment: body.comment,
    recommendation: it.assessment?.recommendation ?? null,
    via,
    carried_from: null,
    supersedes: it.decisions.at(-1)?.id ?? null,
    head: HEAD,
    decided_by: "alice",
    decided_at: new Date().toISOString(),
    effects: planned.map((p, i) =>
      effect(id, i + 1, {
        kind: p.kind,
        state: i === 0 ? "open" : "waiting",
        updated_at: new Date().toISOString(),
      }),
    ),
  };
  it.decisions = [...it.decisions, decided];
  it.state = decided.effects.length ? "executing" : "decided";
  it.revision += 1;
  return decided;
}

/** Why a request is refused, or null (status, code, message). */
export type Refusal = [number, "not_found" | "item_changed" | "invalid_request", string];

/** RC8. */
export function decide(itemId: string, body: DecideRequest): Decision | Refusal {
  items();
  const it = store!.items.find((i) => i.id === itemId);
  if (!it) return [404, "not_found", "This comment is not known for this pull request."];
  if (it.revision !== body.item_revision)
    return [409, "item_changed", `C${it.no} changed since you opened it. Read it again before you decide.`];
  if (body.kind === "reject" && !body.reason)
    return [400, "invalid_request", "A reason is required: it is posted as the reply."];
  return store_decision(it, body, "question");
}

/** RC9: one record per row; gate rows never under group or bulk. */
export function decideBatch(via: "group" | "bulk" | "table", rows: (DecideRequest & { item_no: number })[]) {
  items();
  const found = rows.map((r) => [r, store!.items.find((i) => i.no === r.item_no)] as const);
  if (found.some(([, it]) => !it)) return [404, "not_found", "A listed comment is not known."] as Refusal;
  if (via !== "table" && found.some(([, it]) => it!.gates.length > 0))
    return [422, "invalid_request", "Gate items are decided one at a time."] as Refusal;
  const changed = found.filter(([r, it]) => it!.revision !== r.item_revision).map(([, it]) => `C${it!.no}`);
  if (changed.length)
    return [409, "item_changed", `Changed since you opened them: ${changed.join(", ")}.`] as Refusal;
  return found.map(([r, it]) => store_decision(it!, r, via));
}

/** RC10. */
export function log(): CommentLog {
  const decisions = items()
    .flatMap((i) => i.decisions)
    .sort((a, b) => a.decided_at.localeCompare(b.decided_at));
  return {
    subscribers: [
      {
        token_name: "build-host-2",
        capabilities: ["fix", "reply", "resolve", "follow_up_pr", "upstream_issue"],
        connected: true,
        last_seen_at: at("09:21"),
        push: null,
      },
      {
        token_name: "laptop-cli",
        capabilities: ["reply", "resolve", "dismiss_review"],
        connected: false,
        last_seen_at: at("09:10"),
        push: {
          kind: "ntfy",
          url: "https://ntfy.example.lan",
          topic: "agent-laptop",
          secret_last4: "7c1e",
          delivery: {
            last: { event: null, outcome: "delivered", at: at("09:09") },
            last_error: null,
            failing_since: null,
          },
        },
      },
      {
        token_name: "ci-runner",
        capabilities: ["fix", "reply", "resolve"],
        connected: false,
        last_seen_at: at("08:58"),
        push: {
          kind: "webhook",
          url: "https://ci.example.lan/hooks/diffpilot",
          topic: null,
          secret_last4: null,
          delivery: {
            last: { event: null, outcome: "failed", at: at("09:20") },
            last_error: "HTTP 502 (3 attempts)",
            failing_since: at("09:20"),
          },
        },
      },
    ],
    open_effects: decisions
      .flatMap((d) => d.effects)
      .filter((e) => ["open", "claimed", "waiting"].includes(e.state)).length,
    failed_effects: 0,
    decisions,
    carry_events: carryEvents().filter((e) => e.pr.number === PR.number),
  };
}

function carryEvents(): CarryEvent[] {
  const ev = (
    n: number,
    item_no: number,
    source_item_no: number,
    source_kind: DecisionKind,
    kind: CarryEvent["kind"],
    reason: CarryEvent["reason"],
    when: string,
    pending = 0,
  ): CarryEvent => ({
    id: `c0000000-0000-4000-8000-${String(n).padStart(12, "0")}`,
    pr: PR,
    item_no,
    source_item_no,
    source_decision: `d0000000-0000-4000-8000-${String(source_item_no).padStart(12, "0")}`,
    source_kind,
    kind,
    reason,
    evidence: {
      url: `${URL_PR}#discussion_r10${item_no}`,
      body_hash_before: reason === "bot_edited" ? "4f2a9c" : null,
      body_hash_after: reason === "bot_edited" ? "91be07" : null,
      commit: reason === "code_changed" ? "7d8e9f0a1b2c3d4e5f60718293a4b5c6d7e8f901" : null,
      lines: reason === "code_changed" ? { start_line: 90, end_line: 91 } : null,
    },
    effects_pending: pending,
    at: when,
  });
  return [
    ev(1, 16, 3, "reject", "carried", null, at("08:47")),
    ev(2, 17, 5, "handled_in_pr", "carried", null, at("08:47"), 1),
    ev(3, 21, 3, "reject", "asked_again", "code_changed", at("08:46")),
    ev(4, 13, 2, "fix_here", "asked_again", "bot_edited", at("11:20", "17")),
    ev(5, 10, 4, "reject", "carried", null, at("11:20", "17")),
    ev(6, 7, 1, "upstream_issue", "carried", null, at("15:30", "16")),
  ];
}

const counts0 = (carried: number, edited: number, changed: number, pending: number) => ({
  carried,
  asked_again_bot_edited: edited,
  asked_again_code_changed: changed,
  effects_pending: pending,
});

/** RC11 for one PR (`acme/node-client/482`) or all (null). */
export function carryOvers(pr: string | null): CarryOverPage {
  const events = carryEvents();
  if (pr !== null)
    return {
      totals: counts0(4, 1, 1, 1),
      per_pr: [{ pr: PR, counts: counts0(4, 1, 1, 1) }],
      events,
      next_cursor: null,
    };
  const other = (name: string, number: number) => ({ repo: { owner: "acme", name }, number });
  return {
    totals: counts0(21, 4, 3, 2),
    per_pr: [
      { pr: PR, counts: counts0(4, 1, 1, 1) },
      { pr: other("node-client", 490), counts: counts0(11, 2, 1, 0) },
      { pr: other("node-client", 470), counts: counts0(4, 1, 0, 1) },
      { pr: other("wire-codec", 77), counts: counts0(2, 0, 1, 0) },
    ],
    events,
    next_cursor: null,
  };
}

/** RW2. */
export function watches(): WatchList {
  const other = (name: string, number: number, title: string, fields: Partial<WatchedPr>): WatchedPr => ({
    pr: { repo: { owner: "acme", name }, number },
    title,
    since: at("08:30"),
    last_probe_at: at("09:25"),
    last_auto_pass: null,
    state: "active",
    ended_at: null,
    badge: { badge: null, facts: [], missing: [] },
    ...fields,
  });
  return {
    limit: 20,
    watches: [
      ...(unwatched ? [] : [watched ?? WATCH_482()]),
      other("wire-codec", 77, "refactor(codec): one frame reader", {
        state: "waiting_for_bot_round",
        badge: {
          badge: "waiting_for_bot_round",
          facts: ["thepastaclaw status: reviewing (verify, 2/5), data of 09:01"],
          missing: [],
        },
      }),
      other("node-client", 470, "feat(cli): retry flag", {
        since: at("14:10", "16"),
        last_auto_pass: { at: at("18:02", "17"), questions: 2 },
        badge: { badge: "executing", facts: ["2 effects open"], missing: [] },
      }),
      other("node-client", 474, "fix(codec): length overflow", {
        state: "job_running",
        badge: {
          badge: "blocked_failing",
          facts: ["CI failed at 4c1d9e0", "1 failed effect: reply on C3 (GitHub answered 403)"],
          missing: [],
        },
      }),
      other("node-client", 455, "docs: retry policy", {
        since: at("10:00", "14"),
        last_probe_at: null,
        state: "ended",
        ended_at: at("16:00", "17"),
      }),
    ],
  };
}

let channels: NotificationSettings | null = null;

/** RN1 (seeded: ntfy delivering, webhook failing). */
export function notificationSettings(): NotificationSettings {
  channels ??= {
    channels: [
      {
        kind: "ntfy",
        url: "https://ntfy.example.lan",
        topic: "diffpilot-alice",
        secret_last4: "9f2a",
        events: ["questions_ready", "round_executed"],
        enabled: true,
        delivery: {
          last: { event: "questions_ready", outcome: "delivered", at: at("09:12") },
          last_error: null,
          failing_since: null,
        },
      },
      {
        kind: "webhook",
        url: "http://ha.example.lan:8123/api/webhook/diffpilot-questions",
        topic: null,
        secret_last4: null,
        events: ["questions_ready"],
        enabled: true,
        delivery: {
          last: { event: "questions_ready", outcome: "failed", at: at("09:22") },
          last_error: "connection refused (3 attempts)",
          failing_since: at("09:22"),
        },
      },
    ],
  };
  return channels;
}

/** RN2 / RN3 state changes. */
export function setChannels(next: NotificationSettings): void {
  channels = next;
}

/** Back to the seeded channels (screen tests). */
export function resetChannels(): void {
  channels = null;
}
