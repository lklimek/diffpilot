// Decisions on review comments (plan docs/review-workflow-plan.md §2.3): labels, options, inputs, effects and
// reply texts. Pure functions shared by the question card, the batch pages and the mocks.
import type {
  CommentItem,
  DecideRequest,
  DecisionKind,
  DecisionTarget,
  EffectKind,
  Executor,
  Gate,
} from "../../api/generated/types";

/** A choice on a card: a decision kind, or D8 "Accept despite the gate" (stored as Reject with a gate override). */
export type Choice = DecisionKind | "accept";

/** Visible name of each choice. */
export const CHOICE_LABEL: Record<Choice, string> = {
  resolve: "Resolve (already fixed)",
  fix_here: "Fix here",
  reject: "Reject",
  handled_in_pr: "Handled in another PR",
  follow_up_pr: "Create follow-up PR",
  upstream_issue: "Upstream issue",
  todo: "TODO",
  accept: "Accept despite the gate",
  merge_with_follow_ups: "Merge with the remaining items as follow-ups",
  another_round: "Another round",
  dismiss_stale_reviews: "Dismiss the listed reviews",
  keep_reviews: "Keep them",
};

/** Thread and body item decisions in their base order (D2, D3, D4, D5, D6, D7, D1). */
const ITEM_CHOICES: Choice[] = [
  "fix_here",
  "reject",
  "handled_in_pr",
  "follow_up_pr",
  "upstream_issue",
  "todo",
  "resolve",
];

/** The gate of an item (assessment or owner mark), or null. */
export const gateOf = (item: Pick<CommentItem, "gates">): Gate | null => item.gates[0]?.gate ?? null;

/** The item's options: the recommended one first (RQ7), D8 last on gate items. */
export function choicesFor(item: Pick<CommentItem, "gates" | "assessment">): Choice[] {
  const recommended = item.assessment?.recommendation.kind;
  const rest = ITEM_CHOICES.filter((c) => c !== recommended);
  const first = recommended && ITEM_CHOICES.includes(recommended) ? [recommended] : [];
  return [...first, ...rest, ...(gateOf(item) ? (["accept"] as const) : [])];
}

/** What a choice needs typed in. */
export type Need = "reason" | "pr" | "title" | "upstream" | "todo" | null;

/** The input a choice needs (D3, D8 reason; D4 PR; D5 optional title; D6 URL or "create"; D7 title). */
export function needOf(choice: Choice): Need {
  switch (choice) {
    case "reject":
    case "accept":
      return "reason";
    case "handled_in_pr":
      return "pr";
    case "follow_up_pr":
      return "title";
    case "upstream_issue":
      return "upstream";
    case "todo":
      return "todo";
    default:
      return null;
  }
}

/** D1 and D8 always resolve: "Resolve this thread" is ticked and locked. */
export const resolveLocked = (choice: Choice) => choice === "resolve" || choice === "accept";

/** The values a decision form holds. */
export interface DecisionInput {
  choice: Choice;
  /** Reason (D3, D8), PR number (D4), title (D5, D7) or URL / "create" (D6). */
  value: string;
  resolve: boolean;
  comment: string;
}

/** Why the input cannot be submitted yet, or null. */
export function missingInput(input: Pick<DecisionInput, "choice" | "value">): string | null {
  const value = input.value.trim();
  switch (needOf(input.choice)) {
    case "reason":
      return value ? null : "A reason is required: it is posted as the reply.";
    case "pr":
      return /^#?\d+$/.test(value) ? null : "Enter the number of the existing pull request, e.g. #490.";
    case "upstream":
      return value ? null : 'Enter the upstream issue URL, or "create".';
    case "todo":
      return value ? null : "Enter the TODO title.";
    default:
      return null;
  }
}

/** The target of a choice from its input. */
function targetOf(choice: Choice, value: string): DecisionTarget | null {
  const text = value.trim();
  switch (choice) {
    case "handled_in_pr":
      return { type: "existing_pr", number: Number(text.replace("#", "")) };
    case "follow_up_pr":
      return { type: "follow_up_pr", title: text || null };
    case "upstream_issue":
      return { type: "upstream_issue", url: text.toLowerCase() === "create" ? null : text };
    case "todo":
      return { type: "todo", title: text };
    default:
      return null;
  }
}

/** The request of a decision on `item`; `reply` is the edited reply text, null for the server's default. */
export function requestOf(
  item: Pick<CommentItem, "gates" | "revision">,
  input: DecisionInput,
  reply: string | null,
): DecideRequest {
  const gate = gateOf(item);
  const reason = needOf(input.choice) === "reason" ? input.value.trim() : null;
  return {
    kind: input.choice === "accept" ? "reject" : input.choice,
    reason,
    target: targetOf(input.choice, input.value),
    override: input.choice === "accept" && gate ? { type: "blocker_gate", gate } : null,
    resolve_ticked: resolveLocked(input.choice) || input.resolve,
    reply_text: reply,
    comment: input.comment.trim() || null,
    item_revision: item.revision,
  };
}

/** The reply posted for a choice (English, "per maintainer decision", WM §6.5); null when nothing is posted. */
export function replyText(choice: Choice, value: string): string | null {
  const text = value.trim();
  switch (choice) {
    case "fix_here":
      return "Per maintainer decision, fixed in this pull request. (The commit link follows.)";
    case "reject":
      return `Per maintainer decision, not changed here: ${text}`;
    case "accept":
      return `Per maintainer decision, accepted for this pull request: ${text}`;
    case "handled_in_pr":
      return `Per maintainer decision, this is addressed in #${text.replace("#", "") || "N"}.`;
    case "follow_up_pr":
      return "Per maintainer decision, moved to a new follow-up pull request. (Its link is added when it exists.)";
    case "upstream_issue":
      return "Per maintainer decision, tracked upstream. (The link follows.)";
    case "todo":
      return "Per maintainer decision, out of scope for this PR.";
    default:
      return null;
  }
}

/** One planned effect: kind and executor (plan §2.3; A = external agent, E = agent until phase 3). */
export interface PlannedEffect {
  kind: EffectKind;
  executor: Executor;
  /** What the executor does, after its name. */
  text: string;
}

// TODO(WP13-A CCR): no contract field says whether diffpilot[bot] executes (phase 3); plannedEffects always uses `agent`.
const EXECUTOR_NAME: Record<Executor, string> = { agent: "External agent", diffpilot_bot: "diffpilot[bot]" };

/** Name of an executor. */
export const executorName = (executor: Executor) => EXECUTOR_NAME[executor] ?? executor;

/**
 * The effects of a choice in order. Threads get replies and resolves; body and PR-comment items get one PR-level
 * comment and nothing to resolve.
 */
export function plannedEffects(choice: Choice, resolve: boolean, thread: boolean): PlannedEffect[] {
  const reply = (text: string): PlannedEffect =>
    thread
      ? { kind: "reply", executor: "agent", text }
      : { kind: "pr_comment", executor: "agent", text: `${text} (in one PR-level comment)` };
  const resolves =
    thread && (resolve || resolveLocked(choice))
      ? [{ kind: "resolve" as const, executor: "agent" as const, text: "resolves the thread." }]
      : [];
  switch (choice) {
    case "fix_here":
      return [
        { kind: "fix", executor: "agent", text: "fixes it and pushes." },
        reply("replies after the push with the commit."),
        ...resolves,
      ];
    case "reject":
      return [reply("replies with your reason."), ...resolves];
    case "accept":
      return [reply("replies with your reason (required)."), ...resolves];
    case "handled_in_pr":
      return [reply('replies: "Per maintainer decision, this is addressed in #N."'), ...resolves];
    case "follow_up_pr":
      return [
        {
          kind: "follow_up_pr",
          executor: "agent",
          text: "creates a new draft follow-up PR (no link needed; it does not exist yet).",
        },
        reply("replies with the new PR's link."),
        ...resolves,
      ];
    case "upstream_issue":
      return [
        {
          kind: "upstream_issue",
          executor: "agent",
          text: 'links or creates the upstream issue and adds a "Known limitation" line.',
        },
        reply("replies with the link."),
        ...resolves,
      ];
    case "todo":
      return [
        { kind: "todo", executor: "agent", text: "saves a TODO (never mentioned on GitHub)." },
        reply('replies: "Per maintainer decision, out of scope for this PR."'),
        ...resolves,
      ];
    case "resolve":
      return thread ? [reply("replies what was done."), ...resolves] : [reply("replies what was done.")];
    case "merge_with_follow_ups":
      return [
        {
          kind: "follow_up_pr",
          executor: "agent",
          text: "creates one follow-up PR listing the remaining items.",
        },
        { kind: "reply", executor: "agent", text: "replies on each item with its link." },
        { kind: "resolve", executor: "agent", text: "resolves each thread." },
      ];
    case "dismiss_stale_reviews":
      return [{ kind: "dismiss_review", executor: "agent", text: "dismisses each listed review." }];
    default:
      return [];
  }
}

/** Short words for an effect kind (tables, the log). */
export const EFFECT_WORD: Record<EffectKind, string> = {
  fix: "fix",
  reply: "reply",
  resolve: "resolve",
  pr_comment: "PR-level comment",
  follow_up_pr: "new draft PR",
  upstream_issue: "upstream issue",
  todo: "TODO",
  dismiss_review: "dismiss review",
};

/** The label of a stored decision: D8 for a Reject with a blocker-gate override. */
export function decisionLabel(decision: {
  kind: DecisionKind;
  override: { type: string; gate?: Gate } | null;
  target: DecisionTarget | null;
}): string {
  if (decision.kind === "reject" && decision.override?.type === "blocker_gate")
    return `${CHOICE_LABEL.accept} (${decision.override.gate})`;
  if (decision.kind === "handled_in_pr" && decision.target?.type === "existing_pr")
    return `Handled in #${decision.target.number}`;
  return CHOICE_LABEL[decision.kind] ?? decision.kind;
}

/** The input a recommendation pre-fills: its reason or target. */
export function recommendedValue(item: Pick<CommentItem, "assessment">): string {
  const rec = item.assessment?.recommendation;
  if (!rec) return "";
  const target = rec.target;
  if (target?.type === "existing_pr") return `#${target.number}`;
  if (target?.type === "follow_up_pr") return target.title ?? "";
  if (target?.type === "upstream_issue") return target.url ?? "create";
  if (target?.type === "todo") return target.title;
  return rec.kind === "reject" ? rec.reason : "";
}
