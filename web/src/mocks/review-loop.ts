// The review loop as the mocks' server holds it (docs/review-workflow-plan.md, mockup docs/mockups/10-review-loop.html):
// the PR page, items with assessments and decisions, the log, rounds, carry-overs, watches and notification
// channels of the fixture PR acme/node-client #482. Decisions change the store until `resetReviewLoop`.
// Mockup states are selected with scenarios (`comments` of scenarios.ts).
// Contract gap left to WP13-WATCH: the watched list has no daily automatic-pass count.
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
  DecideRest,
  Decision,
  DecisionBatchRow,
  DecisionKind,
  Effect,
  EffectsPreview,
  Gate,
  GateMarkRequest,
  NotificationSettings,
  PrBadgeView,
  PrRef,
  QueueEntry,
  Recommendation,
  WatchedPr,
  WatchList,
} from "../api/generated/types";
import {
  asReviewComment,
  type CommentItemResponse,
  type ReviewComment,
  type ReviewCommentTally,
  type ReviewCommentsOverview,
  type ReviewPrPage,
} from "../api/provisional/comments";
import { duplicatePrimary, followsPrimary } from "../components/comments/queueStatus";
import {
  ASKED_AGAIN,
  NOT_READ,
  decideRest,
  dismissal,
  followedUp,
  plan,
  planDigest,
  questionOf,
  staleReviews,
  waiting,
  type BotChangeRequest,
  type PlanScope,
  type Planned,
} from "./effects-plan";
import { liveAnalysis } from "./live-analysis";
import { scenario } from "./scenarios";

/** Mockup states of the review loop (scenario field `comments`). */
export type CommentsMockState =
  "no-analysis" | "running" | "empty" | "no-agent" | "github-error" | "watched" | "v14";

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
    skills_commit: "9f3c2ab51e7d40c6a8b2f0e1d3c4b5a697887766",
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
function item(
  no: number,
  fields: Partial<ReviewComment> & Pick<CommentItem, "title" | "body">,
): ReviewComment {
  const built: ReviewComment = {
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
  // RC4: a comment of thepastaclaw's round review is written in that round, on its head at its end.
  const round = built.author === "thepastaclaw" && ROUNDS.find((r) => r.number === built.first_round);
  return round && !("written" in fields)
    ? { ...built, written: { round: round.number, commit: round.head, at: round.finished_at } }
    : built;
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

function nextDecisionId(): string {
  store!.decided += 1;
  return `e0000000-0000-4000-8000-${String(store!.decided).padStart(12, "0")}`;
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
  reply_text: "Per maintainer decision, accepted despite G-FUNDS: Pre-existing fee display; tracked in #455.",
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
  reply_text: "Per maintainer decision, this is addressed in #490.",
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
  reply_text: "Per maintainer decision, not changed: The retry is idempotent by design.",
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
  reply_text: "Per maintainer decision, this is addressed in #490.",
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

function seedItems(): ReviewComment[] {
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
      title: "Merge now, or another round?",
      body: "Bot round 3 has ended. Merge with the remaining items as follow-ups (one follow-up pull request), or another round.",
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
      written: { commit: HEAD, at: "2026-11-17T15:20:00Z" },
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

function v14Items(): ReviewComment[] {
  const seeded = seedItems();
  const primary = seeded.find((i) => i.no === 14)!;
  primary.llm_title = primary.title;
  primary.title = "Medium severity finding";
  return [
    ...seeded,
    item(30, {
      duplicate_of: 14,
      decision_follows_primary: true,
      author: "coderabbitai[bot]",
      title: "The timeout can repeat the request",
      body: "The retry can send a request that was already accepted.",
    }),
    item(31, {
      duplicate_of: 14,
      decision_follows_primary: true,
      title: "The same request can be sent twice",
      body: "The resend may duplicate an accepted request.",
    }),
    item(32, {
      kind: "pr_comment",
      location: null,
      status_comment: true,
      title: "Review round 4 finished",
      body: "The review bot has published its summary.",
    }),
    item(33, {
      state: "decided",
      title: "Use a seeded RNG for the jitter",
      body: "The jitter should use a fixed seed.",
      imported_decision: {
        kind: "reject",
        by: "alice",
        at: at("08:50"),
        evidence_url: `${URL_PR}#discussion_r1038`,
      },
    }),
    item(34, {
      state: "assessing",
      assessment_state: { state: "running" },
      title: "Jitter range documented twice",
      body: "The range is described in two places.",
    }),
    item(35, {
      ...asReviewComment({
        ...item(35, { state: "assessing", title: "Unused import", body: "The import may be unused." }),
        assessment_state: { state: "queued" },
      }),
    }),
    item(36, {
      assessment_state: { state: "not_assessed", reason: "daily_limit" },
      title: "Default changed without changelog",
      body: "The changelog omits the new default.",
    }),
    item(37, {
      assessment_state: { state: "not_assessed", reason: "failed" },
      title: "Check timeout units",
      body: "The timeout unit is unclear.",
    }),
  ];
}

export function commentTally(): ReviewCommentTally {
  const list = listItems();
  return {
    total: list.filter((item) => item.kind !== "pr_question" && !item.gone_at).length,
    needs_you: waitingIn(list).length,
  };
}

export function commentItemsResponse(): CommentItemResponse[] {
  return listItems().map(({ assessment_queued, ...item }) => ({
    ...item,
    assessment_state: assessment_queued ? { state: "queued" } : item.assessment_state,
  }));
}

/** Items, decisions stored so far and the reader's skips ("Skip for now": item → revision skipped at). */
let store: { items: ReviewComment[]; decided: number; skips: Map<number, number> } | null = null;
const items = (): ReviewComment[] => {
  store ??= { items: state() === "v14" ? v14Items() : seedItems(), decided: 0, skips: new Map() };
  return store.items;
};

/** Back to the seeded items (screen tests). */
export function resetReviewLoop(): void {
  store = null;
}

const OPEN_STATES = new Set(["undecided", "reopened", "asked_back"]);
/** Waiting for a decision: open and still produced by the read (an item with `gone_at` leaves the queue). */
const isOpen = (i: CommentItem) => OPEN_STATES.has(i.state) && !i.gone_at;
const isAssessing = (i: CommentItem) => i.state === "assessing" && !i.gone_at;
/** The open items of `list` no standing "Merge with follow-ups" covers (queue.rs `waiting`). */
function waitingIn(list: ReviewComment[]): ReviewComment[] {
  const covered = followedUp(list);
  return list.filter(
    (item) =>
      (isOpen(item) || (duplicatePrimary(item) != null && item.gates.length > 0 && isAssessing(item))) &&
      !covered.has(item.no) &&
      !item.status_comment &&
      !item.imported_decision &&
      !followsPrimary(item) &&
      (item.kind === "pr_question" ||
        duplicatePrimary(item) != null ||
        item.assessment_state.state === "assessed"),
  );
}

/** Items as RC4 answers them in the selected state. */
export function listItems(): ReviewComment[] {
  const all = items();
  applyStandingDecisions(all);
  switch (state()) {
    case "empty":
    case "no-agent":
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

/** Queue order (plan §2.4) with the reader's skipped entries last (RC14). */
function queue(list: ReviewComment[]): QueueEntry[] {
  const skipped = (e: QueueEntry) =>
    e.item_nos.every((n) => store?.skips.get(n) === list.find((i) => i.no === n)?.revision);
  const entries = ordered(list).map((e) => (skipped(e) ? { ...e, skipped: true } : e));
  return [...entries.filter((e) => !e.skipped), ...entries.filter((e) => e.skipped)];
}

/** Queue order (plan §2.4): PR-level, asked again, answered, gate, blocking, single, groups. */
function ordered(list: ReviewComment[]): QueueEntry[] {
  const open = waitingIn(list);
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

function counts(list: ReviewComment[]) {
  const effects = list.flatMap((i) => i.decisions.at(-1)?.effects ?? []);
  return {
    unresolved_threads: list.filter((i) => i.kind === "thread" && i.github.resolved === false).length,
    undecided: waitingIn(list).length,
    asked_again: waitingIn(list).filter((i) => i.state === "reopened").length,
    waiting_for_agents: list.filter((i) => i.state === "executing").length,
    failed_effects: effects.filter((e) => e.state === "failed").length,
    outside: list.filter((i) => i.state === "outside").length,
    total: list.length,
  };
}

function assessmentProgress(list: ReviewComment[]) {
  const needs = list.filter(
    (i) =>
      i.kind !== "pr_question" &&
      !i.status_comment &&
      !i.imported_decision &&
      i.state !== "outside" &&
      !i.gone_at,
  );
  const assessed = needs.filter((i) => i.assessment_state.state === "assessed").length;
  const in_pass = needs.filter((i) => i.assessment_state.state === "running").length;
  const waiting = needs.filter((i) => i.assessment_queued).length;
  return {
    running: in_pass > 0,
    in_pass,
    waiting,
    since: in_pass > 0 ? at("09:18") : undefined,
    assessed,
    total: needs.length,
    limited: needs.filter(
      (i) => i.assessment_state.state === "not_assessed" && i.assessment_state.reason === "daily_limit",
    ).length,
    head: HEAD,
    at: at("09:12"),
  };
}

function badge(list: ReviewComment[]): PrBadgeView {
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
export function overview(): ReviewCommentsOverview {
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
    watch: watchOfPr() ?? undefined,
  };
}

let watched: WatchedPr | null = null;
/** The reader unwatched the fixture PR (it leaves the list). */
let unwatched = false;
const watchOfPr = () => watched ?? (state() === "watched" && !unwatched ? WATCH_482() : null);

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
export function prPage(): ReviewPrPage {
  const list = listItems();
  return {
    pr: PR,
    title: "feat(rpc): retry with backoff on timeout",
    author: "octo-dev",
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
    watch: watchOfPr(),
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
          {
            flow: "f1",
            flow_position: 1,
            flow_title: "Send an RPC request with retry",
            step: "f1.2",
            step_number: 2,
          },
          {
            flow: "f5",
            flow_position: 5,
            flow_title: "Move of backoff helpers util/backoff.rs → rpc/retry.rs",
            step: "f5.1",
            step_number: 1,
          },
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

/** Bots whose latest review on the fixture PR requests changes (the read's `bot_change_requests`). */
export const BOT_CHANGE_REQUESTS: BotChangeRequest[] = [
  { login: "thepastaclaw", review_id: "PRR_round3", url: `${URL_PR}#pullrequestreview-3` },
];

/** What a PR-level decision acts on while `deciding` are decided with it (mod.rs `Loaded::scope`): P1 the waiting
 * items except gate items, which keep their own decision. */
const scopeOf = (deciding: number[]): PlanScope => ({
  number: PR.number,
  remaining: waiting(items()).filter(
    (i) => i.kind !== "pr_question" && i.gates.length === 0 && !deciding.includes(i.no),
  ),
  staleReviews: [],
});

/** A stored decision for `it` as the server plans it (RC8, RC9): effects and the reply text that gets posted. */
function store_decision(
  it: CommentItem,
  body: DecideRequest,
  planned: Planned,
  via: Decision["via"],
  carriedFrom: Decision["id"] | null = null,
): Decision {
  const id = nextDecisionId();
  const decided: Decision = {
    id,
    item_no: it.no,
    kind: body.kind,
    reason: body.reason?.trim() || null,
    target: body.target,
    override: body.override,
    resolve_ticked: body.resolve_ticked,
    reply_text: planned.reply_text,
    comment: body.comment?.trim() || null,
    recommendation: it.assessment?.recommendation ?? null,
    via,
    carried_from: carriedFrom,
    supersedes: it.decisions.at(-1)?.id ?? null,
    head: HEAD,
    decided_by: "alice",
    decided_at: new Date().toISOString(),
    effects: planned.effects.map((p) =>
      effect(id, p.seq, {
        kind: p.kind,
        after: p.after,
        executor: p.executor,
        params: p.params,
        state: p.after === null ? "open" : "waiting",
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
export type Refusal = [number, "not_found" | "item_changed" | "invalid_request" | "conflict", string];

/** The item of a path's `{item}` (`C<no>`, or a bare number), as the server reads it. */
export function itemOfPath(raw: string): CommentItem | undefined {
  items();
  const no = Number(raw.replace(/^[Cc]/, ""));
  return store!.items.find((i) => i.no === no);
}

const NOT_FOUND: Refusal = [404, "not_found", "This comment is not known for this pull request."];
const changedSince = (it: CommentItem): Refusal => [
  409,
  "item_changed",
  `C${it.no} changed since you opened it. Read it again.`,
];
const now = () => new Date().toISOString();
/** Effects of `decision` not yet claimed are cancelled. */
const cancelUnclaimed = (decision: Decision) => {
  decision.effects = decision.effects.map((e) =>
    e.state === "waiting" || e.state === "open"
      ? { ...e, state: "cancelled" as const, updated_at: now() }
      : e,
  );
};

/** RC12: the standing decision is undone while none of its effects is claimed; the item is asked again. */
export function undo(raw: string, revision: number): CommentItem | Refusal {
  const it = itemOfPath(raw);
  if (!it) return NOT_FOUND;
  if (it.revision !== revision) return changedSince(it);
  const last = it.decisions.at(-1);
  if (!last || last.undone_at) return [409, "conflict", `C${it.no} has no decision to undo.`];
  const linked = store!.items.flatMap((duplicate) => {
    const decision = duplicate.decisions.at(-1);
    return decision?.carried_from === last.id && !decision.undone_at ? [{ item: duplicate, decision }] : [];
  });
  const affected = [{ item: it, decision: last }, ...linked];
  if (
    affected.some(({ decision }) =>
      decision.effects.some((e) => e.state === "claimed" || e.state === "done" || e.state === "failed"),
    )
  )
    return [409, "conflict", `An agent already claimed an effect of the decision on C${it.no}; it stays.`];
  const at = now();
  for (const { item, decision } of affected) {
    cancelUnclaimed(decision);
    decision.undone_at = at;
    item.state = "undecided";
    item.revision += 1;
  }
  return it;
}

/** RC13: a decided or carried item is asked again; effects not yet claimed are cancelled. */
export function reopen(raw: string, revision: number): CommentItem | Refusal {
  const it = itemOfPath(raw);
  if (!it) return NOT_FOUND;
  if (it.revision !== revision) return changedSince(it);
  const last = it.decisions.at(-1);
  if (!last || last.undone_at || it.state === "outside")
    return [409, "conflict", `C${it.no} has no decision to reopen.`];
  cancelUnclaimed(last);
  it.state = "reopened";
  it.revision += 1;
  return it;
}

/** RC14: "Skip for now" of the reader at the item's revision. */
export function skip(raw: string, revision: number): Refusal | null {
  const it = itemOfPath(raw);
  if (!it) return NOT_FOUND;
  if (it.revision !== revision) return changedSince(it);
  store!.skips.set(it.no, revision);
  return null;
}

/** RC15: the owner marks or clears his mark of a gate; assessment gates stay. */
export function markGate(raw: string, body: GateMarkRequest): CommentItem | Refusal {
  const it = itemOfPath(raw);
  if (!it) return NOT_FOUND;
  if (it.kind === "pr_question") return [409, "conflict", "A PR-level question has no gates."];
  if (it.revision !== body.item_revision) return changedSince(it);
  const kept = it.gates.filter((g) => !(g.gate === body.gate && g.source === "owner"));
  it.gates = body.marked ? [...kept, { gate: body.gate, source: "owner" }] : kept;
  it.revision += 1;
  return it;
}

type PlannedDuplicate = { item: ReviewComment; body: DecideRequest; planned: Planned };

function planDuplicate(
  item: ReviewComment,
  decision: Pick<DecideRequest, "kind" | "reason" | "target" | "resolve_ticked" | "comment">,
  scope: PlanScope,
): PlannedDuplicate | Refusal {
  const { kind, reason, target, resolve_ticked, comment } = decision;
  const body: DecideRequest = {
    kind,
    reason,
    target,
    resolve_ticked,
    comment,
    item_revision: item.revision,
    override: null,
    reply_text: null,
  };
  const planned = plan(item, body, scope);
  return typeof planned === "string"
    ? [400, "invalid_request", `C${item.no}: ${planned}.`]
    : { item, body, planned };
}

function applyStandingDecisions(all: ReviewComment[]) {
  for (const item of all) {
    if (
      !followsPrimary(item) ||
      (!isOpen(item) && !isAssessing(item)) ||
      item.imported_decision ||
      item.status_comment
    )
      continue;
    const primary = all.find((candidate) => candidate.no === item.duplicate_of);
    const decision = primary?.decisions.at(-1);
    if (
      !primary ||
      isOpen(primary) ||
      isAssessing(primary) ||
      !decision ||
      decision.undone_at ||
      item.decisions.at(-1)?.carried_from === decision.id
    )
      continue;
    const copy = planDuplicate(item, decision, scopeOf([item.no]));
    if (isRefused(copy)) {
      item.decision_follows_primary = false;
      continue;
    }
    store_decision(item, copy.body, copy.planned, "carried", decision.id);
  }
}

type PlannedRow = [CommentItem, Planned, string | null, PlannedDuplicate[]];

/** The server's plans of `rows` (all or nothing) with their plan digests, or its refusal (mod.rs `Loaded::plan`). */
function planRows(rows: DecisionBatchRow[]): PlannedRow[] | Refusal {
  const found = rows.map((r) => [r, store!.items.find((i) => i.no === r.item_no)] as const);
  if (found.some(([, it]) => !it)) return NOT_FOUND;
  const selected = new Set(rows.map((r) => r.item_no));
  const scope = scopeOf([...selected]);
  const out: PlannedRow[] = [];
  for (const [row, it] of found) {
    let at = scope;
    if (row.kind === "dismiss_stale_reviews" && questionOf(it!) === "dismiss") {
      const listed = dismissal(it!, staleReviews(BOT_CHANGE_REQUESTS, store!.items));
      if (typeof listed === "string") return [400, "invalid_request", `C${it!.no}: ${listed}.`];
      at = { ...scope, staleReviews: listed };
    }
    const planned = plan(it!, row, at);
    if (typeof planned === "string") return [400, "invalid_request", `C${it!.no}: ${planned}.`];
    const duplicates: PlannedDuplicate[] = [];
    for (const item of store!.items) {
      if (
        !followsPrimary(item) ||
        item.duplicate_of !== it!.no ||
        selected.has(item.no) ||
        item.imported_decision ||
        item.status_comment ||
        item.gone_at
      )
        continue;
      const carried = item.decisions.at(-1)?.carried_from;
      if (!isOpen(item) && !isAssessing(item) && !it!.decisions.some((decision) => decision.id === carried))
        continue;
      const copy = planDuplicate(item, row, scope);
      if (isRefused(copy)) return copy;
      duplicates.push(copy);
    }
    out.push([it!, planned, planDigest(it!, row, at), duplicates]);
  }
  return out;
}

/** The answer is a refusal. */
export const isRefused = (value: unknown): value is Refusal =>
  Array.isArray(value) && typeof value[0] === "number";

/** RC16: the effects each row would get; nothing is stored. */
export function previewEffects(rows: DecisionBatchRow[]): EffectsPreview[] | Refusal {
  items();
  const planned = planRows(rows);
  if (isRefused(planned)) return planned;
  return planned.flatMap(([it, p, digest, duplicates]) => [
    { item_no: it.no, effects: p.effects, ...(digest === null ? {} : { plan_digest: digest }) },
    ...duplicates.map(({ item, planned }) => ({ item_no: item.no, effects: planned.effects })),
  ]);
}

/** RC17. */
export const decideRestList = (): DecideRest =>
  decideRest(
    listItems().filter((item) => !item.status_comment && !followsPrimary(item) && !item.imported_decision),
    scopeOf,
  );

/** RC8, RC9 (mod.rs `decide`): one record per row, all or nothing; gate rows never under group or bulk, items
 * asked again or no longer read never in bulk. */
function storeRows(via: Decision["via"], rows: DecisionBatchRow[]): Decision[] | Refusal {
  items();
  const twice = rows.find((r, i) => rows.findIndex((o) => o.item_no === r.item_no) !== i);
  if (twice)
    return [400, "invalid_request", `C${twice.item_no}: this decision is not offered for this item.`];
  const own = rows
    .map((r) => store!.items.find((i) => i.no === r.item_no))
    .find((i) => i?.gone_at || i?.state === "reopened");
  if (via === "bulk" && own)
    return [400, "invalid_request", `C${own.no}: ${own.gone_at ? NOT_READ : ASKED_AGAIN}.`];
  const gated = rows
    .map((r) => store!.items.find((i) => i.no === r.item_no))
    .filter((i) => i && i.gates.length > 0)
    .map((i) => `C${i!.no}`);
  if ((via === "group" || via === "bulk") && gated.length)
    return [400, "invalid_request", `Gate items need their own decision: ${gated.join(", ")}.`];
  if (via === "table") {
    const missing = waiting(store!.items)
      .filter((i) => i.gates.length > 0 && !rows.some((r) => r.item_no === i.no))
      .map((i) => `C${i.no}`);
    if (missing.length)
      return [400, "invalid_request", `Gate items need their own decision: ${missing.join(", ")}.`];
  }
  const planned = planRows(rows);
  if (isRefused(planned)) return planned;
  // A row sent with the digest the owner confirmed is refused when its plan rests on something else now.
  const replanned = planned
    .filter(([, , digest], i) => (rows[i]!.plan_digest ?? null) !== null && rows[i]!.plan_digest !== digest)
    .map(([it]) => `C${it.no}`);
  if (replanned.length)
    return [
      409,
      "conflict",
      `What the decision would do changed since you confirmed it: ${replanned.join(", ")}. Check the plan again.`,
    ];
  const changed = planned
    .filter(([it], i) => it.revision !== rows[i]!.item_revision)
    .map(([it]) => `C${it.no}`);
  if (changed.length)
    return [
      409,
      "item_changed",
      `These comments changed since you looked: ${changed.join(", ")}. Read them again.`,
    ];
  return planned.map(([it, p, , duplicates], i) => {
    const primary = store_decision(it, rows[i]!, p, via);
    for (const { item, body, planned } of duplicates)
      store_decision(item, body, planned, "carried", primary.id);
    return primary;
  });
}

/** RC8. */
export function decide(raw: string, body: DecideRequest): Decision | Refusal {
  const it = itemOfPath(raw);
  if (!it) return NOT_FOUND;
  const stored = storeRows("question", [{ item_no: it.no, ...body }]);
  return isRefused(stored) ? stored : stored[0]!;
}

/** RC9. */
export const decideBatch = (via: "group" | "bulk" | "table", rows: DecisionBatchRow[]) =>
  storeRows(via, rows);

/** Agents subscribed to the decision feed of the fixture PR (RC10), new objects on every call. */
const subscribers = (): CommentLog["subscribers"] => [
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
];

/** RC10; the `no-agent` scenario has no agent subscribed. */
export function log(): CommentLog {
  const decisions = items()
    .flatMap((i) => i.decisions)
    .sort((a, b) => a.decided_at.localeCompare(b.decided_at));
  return {
    subscribers: state() === "no-agent" ? [] : subscribers(),
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
    passes_today: 7,
    daily_limit: 80,
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
