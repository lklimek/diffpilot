// The server's effect function as the mocks' server runs it: a port of crates/server/src/review_loop/effects.rs
// `plan` (validation, ordered effects, the reply text that gets posted), of mod.rs `stale_reviews` / `dismissal`
// (the reviews P3 dismisses) and of queue.rs `proposed` / `decide_rest` with views.rs `decide_rest` (RC17). It
// models the server, never the web's own helpers, so that web/server drift shows up in tests; keep it in step with
// those files.
import type {
  CommentItem,
  DecideRequest,
  DecideRest,
  DecisionBatchRow,
  DecisionKind,
  EffectKind,
  EffectParams,
  Gate,
  LeftOutItem,
  LeftOutReason,
  PlannedEffect,
} from "../api/generated/types";

const MAX_TEXT_CHARS = 4000;
const FOLLOW_UP_REPLY = "Per maintainer decision, this moves to a follow-up pull request.";
/** effects.rs `DISMISS_MESSAGE`. */
export const DISMISS_MESSAGE = "Per maintainer decision, every item of this review is decided.";

/** effects.rs `DecisionError` as the server words it. */
const ERROR = {
  kindNotOffered: "this decision is not offered for this item",
  reasonRequired: "this decision needs a reason",
  targetRequired: "this decision needs its target",
  targetInvalid: "the target does not fit this decision",
  overrideInvalid: "the override does not fit this decision",
  resolveLocked: "this decision always resolves the thread",
  tooLong: `a text is longer than ${MAX_TEXT_CHARS} characters`,
  reviewsChanged:
    "the reviews this question lists changed since it was asked; keep the reviews, the next round asks again",
  askedAgain: "it was asked again, so it is decided on its own, not in bulk",
  notRead: "it is not in the latest read from GitHub, so it is decided on its own, not in bulk",
} as const;

/** Why a decision does not fit its item (the server's words). */
export type DecisionError = (typeof ERROR)[keyof typeof ERROR];

/** mod.rs `decide`: a bulk batch with an item asked again (`reopened`) is refused. */
export const ASKED_AGAIN: DecisionError = ERROR.askedAgain;
/** mod.rs `decide`: a bulk batch with an item the latest complete read no longer produced (`gone_at`) is refused. */
export const NOT_READ: DecisionError = ERROR.notRead;

/** What a PR-level decision acts on (effects.rs `PrScope`). */
export interface PlanScope {
  number: number;
  /** P1: the items still undecided, gate items left out. */
  remaining: CommentItem[];
  /** P3: the stale bot reviews the question lists (node ids). */
  staleReviews: string[];
}

/** effects.rs `Planned`. */
export interface Planned {
  effects: PlannedEffect[];
  reply_text: string | null;
}

const PR_KINDS: DecisionKind[] = [
  "merge_with_follow_ups",
  "another_round",
  "dismiss_stale_reviews",
  "keep_reviews",
];

/** mod.rs `question_of`: `pr:<round>:merge` or `pr:<round>:dismiss`. */
export function questionOf(item: CommentItem): "merge" | "dismiss" | null {
  if (item.kind !== "pr_question") return null;
  const [pr, round, kind] = item.stable_key.split(":");
  if (pr !== "pr" || !round || (kind !== "merge" && kind !== "dismiss")) return null;
  return kind;
}

// The fixture's GitHub node ids: every thread item has its thread, review-body items their review.
const threadId = (item: CommentItem) => (item.kind === "thread" ? `PRRT_${item.no}` : null);
const reviewId = (item: CommentItem) =>
  item.kind === "review_body" ? `PRR_${item.stable_key.split(":")[0]}` : null;

/**
 * effects.rs `EffectItem::on_review`: a PR-level comment answering a review starts with a line naming the review, a
 * person by an @-mention, a bot without one.
 */
function onReview(item: CommentItem, text: string): string {
  if (item.kind !== "review_body" || !item.url) return text;
  const line =
    item.author_kind === "human"
      ? `@${item.author}, re [your review](${item.url}):`
      : `Re [${item.author}'s review](${item.url}):`;
  return `${line}\n${text}`;
}

const tooLong = (text: string | null | undefined) => !!text && [...text].length > MAX_TEXT_CHARS;
const nonEmpty = (text: string | null | undefined) => text?.trim() || null;

function offered(item: CommentItem, kind: DecisionKind): boolean {
  switch (questionOf(item)) {
    case "merge":
      return kind === "merge_with_follow_ups" || kind === "another_round";
    case "dismiss":
      return kind === "dismiss_stale_reviews" || kind === "keep_reviews";
    default:
      return !PR_KINDS.includes(kind);
  }
}

class Builder {
  effects: PlannedEffect[] = [];

  push(kind: EffectKind, after: number | null, params: Partial<EffectParams>): number {
    const seq = this.effects.length + 1;
    this.effects.push({
      seq,
      after,
      kind,
      executor: "agent",
      params: {
        text: null,
        thread_id: null,
        review_id: null,
        title: null,
        url: null,
        item_nos: [],
        ...params,
      },
    });
    return seq;
  }

  replyAndResolve(item: CommentItem, text: string | null, after: number | null, resolve: boolean) {
    let last = after;
    const thread = threadId(item);
    if (text !== null)
      last = this.push(thread ? "reply" : "pr_comment", last, {
        text,
        thread_id: thread,
        review_id: reviewId(item),
        item_nos: [item.no],
      });
    if (resolve && thread) last = this.push("resolve", last, { thread_id: thread, item_nos: [item.no] });
    return last;
  }
}

/** Validates `request` against `item` and plans its effects (effects.rs `plan`); a string is the refusal. */
export function plan(item: CommentItem, request: DecideRequest, scope: PlanScope): Planned | DecisionError {
  if (!offered(item, request.kind)) return ERROR.kindNotOffered;
  if (tooLong(request.reason) || tooLong(request.reply_text) || tooLong(request.comment))
    return ERROR.tooLong;
  let gate: Gate | null = null;
  const override = request.override;
  if (override?.type === "blocker_gate") {
    if (request.kind !== "reject" || !item.gates.some((g) => g.gate === override.gate))
      return ERROR.overrideInvalid;
    gate = override.gate;
  } else if (override?.type === "necessity_gate" && request.kind !== "fix_here") return ERROR.overrideInvalid;
  const target = request.target;
  if (target === null) {
    if (["handled_in_pr", "upstream_issue", "todo"].includes(request.kind)) return ERROR.targetRequired;
  } else if (target.type === "existing_pr" && request.kind === "handled_in_pr") {
    if (target.number === 0 || target.number === scope.number) return ERROR.targetInvalid;
  } else if (target.type === "follow_up_pr" && request.kind === "follow_up_pr") {
    if (tooLong(target.title)) return ERROR.tooLong;
  } else if (target.type === "upstream_issue" && request.kind === "upstream_issue") {
    if (target.url !== null && !/^https?:\/\/[^\s/]+/i.test(target.url)) return ERROR.targetInvalid;
    if (tooLong(target.url)) return ERROR.targetInvalid;
  } else if (target.type === "todo" && request.kind === "todo") {
    if (!nonEmpty(target.title)) return ERROR.targetRequired;
    if (tooLong(target.title)) return ERROR.tooLong;
  } else return ERROR.targetInvalid;
  const reason = nonEmpty(request.reason);
  if (request.kind === "reject" && !reason) return ERROR.reasonRequired;
  const alwaysResolves = request.kind === "resolve" || gate !== null;
  if (alwaysResolves && threadId(item) && !request.resolve_ticked) return ERROR.resolveLocked;
  const resolve = request.resolve_ticked;
  const edited = nonEmpty(request.reply_text);
  const reply = (fallback: string) => edited ?? onReview(item, fallback);
  const out = new Builder();
  let replyText: string | null;
  switch (request.kind) {
    case "resolve":
      replyText =
        item.author_kind === "human"
          ? reply("Per maintainer decision, this is already addressed in this pull request.")
          : null;
      out.replyAndResolve(item, replyText, null, true);
      break;
    case "fix_here": {
      const fix = out.push("fix", null, { thread_id: threadId(item), item_nos: [item.no] });
      replyText = reply("Per maintainer decision, this is fixed in this pull request.");
      out.replyAndResolve(item, replyText, fix, resolve);
      break;
    }
    case "reject":
      replyText = reply(
        gate
          ? `Per maintainer decision, accepted despite ${gate}: ${reason}`
          : `Per maintainer decision, not changed: ${reason}`,
      );
      out.replyAndResolve(item, replyText, null, resolve || gate !== null);
      break;
    case "handled_in_pr": {
      const number = target?.type === "existing_pr" ? target.number : 0;
      replyText = reply(`Per maintainer decision, this is addressed in #${number}.`);
      out.replyAndResolve(item, replyText, null, resolve);
      break;
    }
    case "follow_up_pr": {
      const title = target?.type === "follow_up_pr" ? nonEmpty(target.title) : null;
      const create = out.push("follow_up_pr", null, { title, item_nos: [item.no] });
      replyText = reply(FOLLOW_UP_REPLY);
      out.replyAndResolve(item, replyText, create, resolve);
      break;
    }
    case "upstream_issue": {
      const url = target?.type === "upstream_issue" ? nonEmpty(target.url) : null;
      const issue = out.push("upstream_issue", null, { url, item_nos: [item.no] });
      replyText = reply(
        url
          ? `Per maintainer decision, this is tracked upstream: ${url}`
          : "Per maintainer decision, this is tracked in an upstream issue.",
      );
      out.replyAndResolve(item, replyText, issue, resolve);
      break;
    }
    case "todo": {
      const title = target?.type === "todo" ? target.title.trim() : "";
      const todo = out.push("todo", null, { title, item_nos: [item.no] });
      replyText = reply("Per maintainer decision, out of scope for this PR.");
      out.replyAndResolve(item, replyText, todo, resolve);
      break;
    }
    case "merge_with_follow_ups":
      if (scope.remaining.length > 0) {
        const create = out.push("follow_up_pr", null, { item_nos: scope.remaining.map((i) => i.no) });
        for (const remaining of scope.remaining)
          out.replyAndResolve(remaining, onReview(remaining, FOLLOW_UP_REPLY), create, true);
      }
      replyText = null;
      break;
    case "dismiss_stale_reviews":
      replyText = reason ?? DISMISS_MESSAGE;
      for (const review of scope.staleReviews)
        out.push("dismiss_review", null, { text: replyText, review_id: review });
      break;
    default:
      replyText = null;
  }
  return { effects: out.effects, reply_text: replyText };
}

/**
 * mod.rs `Loaded::plan_digest`: what a PR-level plan rests on ("Merge now" the items it follows up with their
 * revisions, the dismissal the reviews it dismisses) as an opaque digest; null for other rows. The mock hashes with
 * FNV-1a where the server uses SHA-256: clients only compare it.
 */
export function planDigest(item: CommentItem, request: DecideRequest, scope: PlanScope): string | null {
  const restsOn =
    request.kind === "merge_with_follow_ups"
      ? scope.remaining.map((i) => `follow-up C${i.no} r${i.revision}`)
      : request.kind === "dismiss_stale_reviews" && questionOf(item) === "dismiss"
        ? scope.staleReviews.map((review) => `dismiss ${review}`)
        : null;
  if (restsOn === null) return null;
  let hash = 0x811c9dc5;
  for (const char of restsOn.map((line) => `${line}\n`).join(""))
    hash = Math.imul(hash ^ char.charCodeAt(0), 0x01000193) >>> 0;
  return hash.toString(16).padStart(8, "0");
}

const OPEN = new Set(["undecided", "assessing", "asked_back", "reopened"]);

/** The item's standing decision: its latest one, not undone (mod.rs `standing`). */
const standing = (item: CommentItem) => {
  const last = item.decisions.at(-1);
  return last && !last.undone_at ? last : null;
};

/** mod.rs `Loaded::followed_up`: items listed in the (not cancelled) follow-up PR of a standing P1 decision. */
export function followedUp(items: CommentItem[]): Set<number> {
  return new Set(
    items.flatMap((i) => {
      const decision = standing(i);
      return decision?.kind === "merge_with_follow_ups"
        ? decision.effects
            .filter((e) => e.kind === "follow_up_pr" && e.state !== "cancelled")
            .flatMap((e) => e.params.item_nos)
        : [];
    }),
  );
}

/** queue.rs `waiting` over mod.rs `Loaded::queue_items`: open items the read still produces (no `gone_at`) and no
 * standing P1 decision covers; the fixture has one PR-level question of each kind at most. */
export function waiting(items: CommentItem[]): CommentItem[] {
  const covered = followedUp(items);
  return items.filter((i) => OPEN.has(i.state) && !i.gone_at && !covered.has(i.no));
}

/** observe.rs `BotChangeRequest`: a bot whose latest review requests changes. */
export interface BotChangeRequest {
  login: string;
  review_id: string;
  url: string;
}

/** mod.rs `stale_reviews`: every item the bot raised (not resolved outside) has a standing decision and is not open. */
export function staleReviews(requests: BotChangeRequest[], items: CommentItem[]): BotChangeRequest[] {
  return requests.filter((request) => {
    const raised = items.filter(
      (i) => i.author.toLowerCase() === request.login.toLowerCase() && i.state !== "outside" && !i.gone_at,
    );
    return raised.length > 0 && raised.every((i) => standing(i) !== null && !OPEN.has(i.state));
  });
}

/** mod.rs `dismiss_body`: the "Dismiss stale bot reviews?" text; its list is what deciding P3 dismisses. */
export const dismissBody = (stale: BotChangeRequest[]) =>
  ["Every item these reviews raised has a decision:", ...stale.map((r) => `- ${r.login}: ${r.url}`)].join(
    "\n",
  );

/** mod.rs `Loaded::dismissal`: the reviews `question` lists, each still stale now; else why P3 is refused. */
export function dismissal(question: CommentItem, stale: BotChangeRequest[]): string[] | DecisionError {
  const listed = question.body
    .split("\n")
    .filter((line) => line.startsWith("- ") && line.includes(": "))
    .map((line) => {
      const rest = line.slice(2);
      const at = rest.indexOf(": ");
      return [rest.slice(0, at), rest.slice(at + 2)] as const;
    });
  const ids: string[] = [];
  for (const [login, url] of listed) {
    const found = stale.find((r) => r.login.toLowerCase() === login.toLowerCase() && r.url === url);
    if (!found) return ERROR.reviewsChanged;
    ids.push(found.review_id);
  }
  return ids;
}

/** queue.rs `proposed`: a recommendation as the decision it proposes, or why it cannot be one. */
function proposed(item: CommentItem): DecideRequest | LeftOutReason {
  const rec = item.assessment!.recommendation;
  const reason = rec.kind === "reject" ? rec.reason.trim() || null : null;
  const incomplete =
    rec.kind === "reject"
      ? !reason
      : ["handled_in_pr", "upstream_issue", "todo"].includes(rec.kind)
        ? rec.target === null
        : PR_KINDS.includes(rec.kind);
  if (incomplete) return "incomplete";
  return {
    kind: rec.kind,
    reason,
    target: rec.target,
    override: null,
    resolve_ticked: true,
    reply_text: null,
    comment: null,
    item_revision: item.revision,
  };
}

/** RC17 (views.rs `decide_rest` over queue.rs `decide_rest`); `scopeOf` gives the scope of a planned row. */
export function decideRest(items: CommentItem[], scopeOf: (deciding: number[]) => PlanScope): DecideRest {
  const rows: DecideRest["rows"] = [];
  const leftOut: LeftOutItem[] = [];
  for (const item of waiting(items)
    .filter((i) => i.kind !== "pr_question")
    .sort((a, b) => a.no - b.no)) {
    const outcome: DecideRequest | LeftOutReason =
      item.gates.length > 0
        ? "gate"
        : item.state === "reopened"
          ? "asked_again"
          : !item.assessment
            ? "not_assessed"
            : proposed(item);
    if (typeof outcome === "string") {
      leftOut.push({ item_no: item.no, reason: outcome });
      continue;
    }
    const row: DecisionBatchRow = { item_no: item.no, ...outcome };
    const planned = plan(item, outcome, scopeOf([item.no]));
    if (typeof planned === "string") leftOut.push({ item_no: item.no, reason: "incomplete" });
    else rows.push({ row, recommendation: item.assessment!.recommendation, effects: planned.effects });
  }
  leftOut.sort((a, b) => a.item_no - b.item_no);
  return { rows, left_out: leftOut };
}
