// The Ask chat's threads, subjects and unread answers: plain functions of the questions and the tour.
import type {
  Annotation,
  AskAnchor,
  AskQuestion,
  CommentItemId,
  LineRange,
  LineSide,
  QuestionId,
  Step,
  Tour,
} from "../../../api/generated/types";
import { askOrder, type HistoryQuestion } from "../../../api/questionHistory";
import { annotationName, rangeText } from "../../section/format";

/** The opening line of a draft from a remark; the capture names its lines. */
export const remarkQuote = /^> LLM remark \([^\n]+?, (lines? \d+(?:–\d+)?)\):[^\n]*(?:\n{1,2}|$)/;

const linesLabel = (range: LineRange) =>
  `${range.start_line === range.end_line ? "line" : "lines"} ${rangeText(range)}`;

/** The quoted lines label when it matches the question's selected anchor. */
export function remarkLines(text: string, anchor: AskAnchor): string | undefined {
  const range = anchor.kind === "step" ? anchor.lines : anchor.kind === "lines" ? anchor.range : null;
  const quoted = text.match(remarkQuote)?.[1];
  return range && quoted === linesLabel(range) ? quoted : undefined;
}

/** The quoted context uses the existing plain-text question contract. */
export function quoteRemark(remark: Annotation): string {
  const lines = linesLabel(remark.range);
  const title = (remark.text || remark.is || remark.was).replace(/\s+/g, " ").trim();
  return `> LLM remark (${annotationName(remark)}, ${lines}): ${title}\n\n`;
}

/** A first question with its follow-ups, oldest first. */
export interface ChatThread {
  root: HistoryQuestion;
  followUps: AskQuestion[];
}

const asked = (a: AskQuestion, b: AskQuestion) => Date.parse(a.created_at) - Date.parse(b.created_at);

/** Threads oldest first; a follow-up of a follow-up joins the thread of its first question (one level). */
export function threadsOf(questions: readonly HistoryQuestion[]): ChatThread[] {
  const byId = new Map(questions.map((q) => [q.id, q]));
  const rootOf = (q: HistoryQuestion): HistoryQuestion => {
    const seen = new Set<QuestionId>();
    let at = q;
    while (at.parent_id && byId.has(at.parent_id) && !seen.has(at.id)) {
      seen.add(at.id);
      at = byId.get(at.parent_id)!;
    }
    return at;
  };
  const threads = new Map<QuestionId, ChatThread>();
  for (const q of [...questions].sort(asked)) {
    const root = rootOf(q);
    const thread = threads.get(root.id) ?? { root, followUps: [] };
    if (root.id !== q.id) thread.followUps.push(q);
    threads.set(root.id, thread);
  }
  return [...threads.values()].sort((a, b) => askOrder(a.root.firstAsk ?? a.root, b.root.firstAsk ?? b.root));
}

/** How a subject is named: in a chip, the field's placeholder and the Ask buttons. */
export interface Subject {
  /** Compact chip label. */
  text: string;
  /** Full path, range and side for the tooltip and accessible name. */
  fileLabel?: string;
  /** "this step", "these lines", "this file", "this flow", "the whole PR". */
  what: string;
  /** Tooltip of its chip. */
  tip: string;
}

const findStep = (tour: Tour, id: string): Step | undefined =>
  tour.flows.flatMap((f) => f.steps).find((s) => s.id === id);
const flowOfStep = (tour: Tour, id: string) => tour.flows.find((f) => f.steps.some((s) => s.id === id));
const basename = (path: string) => path.split("/").at(-1) ?? path;
const old = (side: LineSide) => (side === "base" ? " (old code)" : "");
const fileLabel = (path: string, side: LineSide, range: LineRange | null): string =>
  `${path}${range ? `:${rangeText(range)}` : ""} (${side === "base" ? "old" : "new"} code)`;

/** The per-PR numbers (`C<no>`) of the review-comment items the chat knows. */
export type ItemNumbers = ReadonlyMap<CommentItemId, number>;

const NO_NUMBERS: ItemNumbers = new Map();

/** The name of a review-comment item subject: "Comment C14" when its number is known. */
export function commentSubject(item: CommentItemId, numbers: ItemNumbers = NO_NUMBERS): Subject {
  const no = numbers.get(item);
  const name = no === undefined ? "a review comment" : `review comment C${no}`;
  return {
    text: no === undefined ? "Review comment" : `Comment C${no}`,
    what: "this comment",
    tip: `Asked about ${name}. Opens it in Comments; the chat stays open.`,
  };
}

/** The whole pull request as a subject. */
export const WHOLE_PR: Subject = {
  text: "Whole PR",
  what: "the whole PR",
  tip: "Asked about the whole pull request, not one place. Opens the PR context; the chat stays open.",
};

/** The name of what a question is about; `numbers` names review-comment items. */
export function subjectOf(anchor: AskAnchor, tour: Tour, numbers: ItemNumbers = NO_NUMBERS): Subject {
  switch (anchor.kind) {
    case "step": {
      const flow = flowOfStep(tour, anchor.step_id);
      const step = flow?.steps.find((s) => s.id === anchor.step_id);
      const range = anchor.lines ?? step?.facts.range;
      const text =
        flow && step && range
          ? `Step ${flow.position}.${step.number} · ${basename(step.facts.file)} ${rangeText(range)}${old(step.facts.side)}`
          : "A step not in these flows";
      return {
        text,
        fileLabel: step && range ? fileLabel(step.facts.file, step.facts.side, range) : undefined,
        what: anchor.lines ? "these lines" : "this step",
        tip: anchor.lines
          ? "Asked about these lines. Opens them in the Files view; the chat stays open."
          : "Asked on this step. Opens it in the Flows view; the chat stays open.",
      };
    }
    case "lines": {
      const { path, side, range } = anchor;
      return {
        text: `${basename(path)}${range ? ` ${rangeText(range)}` : ""}${old(side)}`,
        fileLabel: fileLabel(path, side, range),
        what: range ? "these lines" : "this file",
        tip: range
          ? "Asked about these lines. Opens them in the Files view; the chat stays open."
          : "Asked about this file. Opens it in the Files view; the chat stays open.",
      };
    }
    case "flow": {
      const flow = tour.flows.find((f) => f.id === anchor.flow_id);
      return {
        text: flow ? `Flow ${flow.position}` : "A flow not in this analysis",
        what: "this flow",
        tip: "Asked about this flow as a whole. Opens its introduction; the chat stays open.",
      };
    }
    case "comment_item":
      return commentSubject(anchor.item, numbers);
    case "pr":
      return WHOLE_PR;
  }
}

/** Lines of a file an anchor covers (`to` null: the whole file); null for a flow or the whole PR. */
export function linesOf(
  anchor: AskAnchor,
  tour: Tour,
): { path: string; side: LineSide; from: number; to: number | null } | null {
  if (anchor.kind === "lines")
    return {
      path: anchor.path,
      side: anchor.side,
      from: anchor.range?.start_line ?? 1,
      to: anchor.range?.end_line ?? null,
    };
  if (anchor.kind !== "step") return null;
  const step = findStep(tour, anchor.step_id);
  if (!step) return null;
  const range = anchor.lines ?? step.facts.range;
  return { path: step.facts.file, side: step.facts.side, from: range.start_line, to: range.end_line };
}

const overlap = (a: { from: number; to: number | null }, b: { from: number; to: number | null }) =>
  (a.to === null || b.from <= a.to) && (b.to === null || a.from <= b.to);

/** Whether a thread's subject belongs to the Inspector copy of `place`. */
function about(anchor: AskAnchor, place: AskAnchor, tour: Tour): boolean {
  switch (place.kind) {
    case "pr":
      return anchor.kind === "pr";
    case "comment_item":
      return anchor.kind === "comment_item" && anchor.item === place.item;
    case "flow":
      return anchor.kind === "flow" && anchor.flow_id === place.flow_id;
    case "step": {
      if (anchor.kind === "step") return anchor.step_id === place.step_id;
      const here = linesOf(place, tour);
      const there = anchor.kind === "lines" && anchor.range ? linesOf(anchor, tour) : null;
      return Boolean(
        here && there && here.path === there.path && here.side === there.side && overlap(here, there),
      );
    }
    case "lines": {
      const there = linesOf(anchor, tour);
      const here = linesOf(place, tour)!;
      return Boolean(there && there.path === here.path && there.side === here.side && overlap(here, there));
    }
  }
}

/** The threads about a place (its Inspector copy), oldest first. */
export function questionsAt(
  questions: readonly HistoryQuestion[],
  place: AskAnchor,
  tour: Tour,
): ChatThread[] {
  return threadsOf(questions).filter((t) => about(t.root.anchor, place, tour));
}

/** Ids of questions with an outcome (answer or failure) not in `seen`, oldest first. */
export function unreadOf(questions: readonly AskQuestion[], seen: ReadonlySet<QuestionId>): QuestionId[] {
  return [...questions]
    .filter((q) => (q.state === "answered" || q.state === "failed") && !seen.has(q.id))
    .sort(asked)
    .map((q) => q.id);
}

/** Whether an answer about the anchor can become a review comment: it has lines to anchor on. */
export const hasLines = (anchor: AskAnchor) =>
  anchor.kind === "step" || (anchor.kind === "lines" && anchor.range !== null);
