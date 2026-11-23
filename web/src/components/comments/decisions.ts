// Decisions on review comments (plan docs/review-workflow-plan.md §2.3): labels, options, inputs and the words for
// the effects the server plans. Pure functions shared by the question card and the batch pages; effects and reply
// texts come from the server (RC16, RC17).
import type {
  CommentItem,
  DecideRequest,
  DecisionKind,
  DecisionTarget,
  EffectKind,
  EffectsPreview,
  Executor,
  Gate,
  PlannedEffect,
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

const EXECUTOR_NAME: Record<Executor, string> = { agent: "External agent", diffpilot_bot: "diffpilot[bot]" };

/** Name of an executor. */
export const executorName = (executor: Executor) => EXECUTOR_NAME[executor] ?? executor;

/** One line of "What happens next": who does what. */
export interface EffectLine {
  executor: Executor;
  /** What the executor does, after its name. */
  text: string;
}

const names = (nos: number[]) => nos.map((no) => `C${no}`).join(", ");

/**
 * What `count` effects of one kind do, in plain words. One item: the reply text is shown below the lines; several
 * items (`on` names them): the text is quoted in the line.
 */
function effectText(kind: EffectKind, params: PlannedEffect["params"], count: number, on: string): string {
  const text = on ? `: "${params.text ?? ""}"` : " with the reply text below.";
  switch (kind) {
    case "fix":
      return `fixes it and pushes${on}.`;
    case "reply":
      return `replies${on}${text}`;
    case "pr_comment":
      return `posts one PR-level comment${on}${text}`;
    case "resolve":
      return count > 1 ? `resolves the threads${on}.` : `resolves the thread${on}.`;
    case "follow_up_pr":
      return params.item_nos.length > 1
        ? `creates one draft follow-up PR listing ${names(params.item_nos)} (no link needed; it does not exist yet).`
        : `creates a new draft follow-up PR${params.title ? ` "${params.title}"` : ""} (no link needed; it does not exist yet).`;
    case "upstream_issue":
      return params.url
        ? `links the upstream issue ${params.url} and adds a "Known limitation" line.`
        : 'creates the upstream issue and adds a "Known limitation" line.';
    case "todo":
      return "saves a TODO (never mentioned on GitHub).";
    case "dismiss_review":
      return count > 1
        ? `dismisses each of the ${count} stale reviews with the message above.`
        : "dismisses the stale review with the message above.";
  }
}

/**
 * The server's planned effects (RC16, RC17) as "What happens next" lines in order: one per kind and text; with
 * `several` items the lines name them and quote the reply texts.
 */
export function effectLines(effects: PlannedEffect[], several = false): EffectLine[] {
  const lines: { first: PlannedEffect; nos: number[]; count: number }[] = [];
  for (const effect of effects) {
    const same = lines.find(
      (l) =>
        l.first.kind === effect.kind &&
        l.first.executor === effect.executor &&
        l.first.params.text === effect.params.text,
    );
    const nos = effect.kind === "follow_up_pr" ? [] : effect.params.item_nos;
    if (same) {
      same.nos.push(...nos.filter((no) => !same.nos.includes(no)));
      same.count += 1;
    } else lines.push({ first: effect, nos: [...nos], count: 1 });
  }
  return lines.map(({ first, nos, count }) => ({
    executor: first.executor,
    text: effectText(first.kind, first.params, count, several && nos.length > 0 ? ` on ${names(nos)}` : ""),
  }));
}

/** The server's plan of a card's draft (RC16): nothing to plan yet (an input is missing), loading, refused, or ready. */
export type EffectsPreviewState =
  | { state: "idle" }
  | { state: "loading" }
  | { state: "error"; message: string }
  | {
      state: "ready";
      items: EffectsPreview[];
      /** A newer draft is being planned; these effects are of the previous one. */
      updating: boolean;
    };

/** The planned effects of item `C<no>` in `preview`, or null while it has none. */
export const previewFor = (preview: EffectsPreviewState | undefined, no: number): PlannedEffect[] | null =>
  preview?.state === "ready" ? (preview.items.find((i) => i.item_no === no)?.effects ?? null) : null;

/** The reply text the server would post (the first reply or PR-level comment of the plan), or null. */
export const plannedReply = (effects: PlannedEffect[]): string | null =>
  effects.find((e) => e.kind === "reply" || e.kind === "pr_comment")?.params.text ?? null;

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
