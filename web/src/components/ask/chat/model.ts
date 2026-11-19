// The Ask chat's threads, subjects and unread answers: plain functions of the questions and the tour.
import type { ChatAnchor, ChatQuestion } from "../../../api/askChat";
import type { LineSide, QuestionId, Step, Tour } from "../../../api/generated/types";
import { rangeText } from "../../section/format";

/** A first question with its follow-ups, oldest first. */
export interface ChatThread {
  root: ChatQuestion;
  followUps: ChatQuestion[];
}

const asked = (a: ChatQuestion, b: ChatQuestion) => Date.parse(a.created_at) - Date.parse(b.created_at);

/** Threads oldest first; a follow-up of a follow-up joins the thread of its first question (one level). */
export function threadsOf(questions: readonly ChatQuestion[]): ChatThread[] {
  const byId = new Map(questions.map((q) => [q.id, q]));
  const rootOf = (q: ChatQuestion): ChatQuestion => {
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
  return [...threads.values()].sort((a, b) => asked(a.root, b.root));
}

/** How a subject is named: in a chip, the field's placeholder and the Ask buttons. */
export interface Subject {
  /** The view its chip opens; null for the whole PR. */
  view: "Flows" | "Files" | null;
  /** `path:lines` (or the path), drawn as code; null without lines. */
  code: string | null;
  text: string;
  /** "this step", "these lines", "this file", "this flow", "the whole PR". */
  what: string;
  /** Tooltip of its chip. */
  tip: string;
}

const findStep = (tour: Tour, id: string): Step | undefined =>
  tour.flows.flatMap((f) => f.steps).find((s) => s.id === id);
const flowOfStep = (tour: Tour, id: string) => tour.flows.find((f) => f.steps.some((s) => s.id === id));
const old = (side: LineSide) => (side === "base" ? " (old code)" : "");

/** The name of what a question is about. */
export function subjectOf(anchor: ChatAnchor, tour: Tour): Subject {
  switch (anchor.kind) {
    case "step": {
      const step = findStep(tour, anchor.step_id);
      const flow = flowOfStep(tour, anchor.step_id);
      const where =
        step && flow ? `flow ${flow.position} · step ${step.number}` : "a step not in these flows";
      if (anchor.lines) {
        return {
          view: "Files",
          code: step ? `${step.facts.file}:${rangeText(anchor.lines)}${old(step.facts.side)}` : null,
          text: where,
          what: "these lines",
          tip: "Asked about these lines. Opens them in the Files view; the chat stays open.",
        };
      }
      return {
        view: "Flows",
        code: step ? `${step.facts.file}:${rangeText(step.facts.range)}${old(step.facts.side)}` : null,
        text: step ? `${where} · ${step.llm.title}` : where,
        what: "this step",
        tip: "Asked on this step. Opens it in the Flows view; the chat stays open.",
      };
    }
    case "lines":
      return anchor.range
        ? {
            view: "Files",
            code: `${anchor.path}:${rangeText(anchor.range)}${old(anchor.side)}`,
            text: "",
            what: "these lines",
            tip: "Asked about these lines. Opens them in the Files view; the chat stays open.",
          }
        : {
            view: "Files",
            code: `${anchor.path}${old(anchor.side)}`,
            text: "whole file",
            what: "this file",
            tip: "Asked about this file. Opens it in the Files view; the chat stays open.",
          };
    case "flow": {
      const flow = tour.flows.find((f) => f.id === anchor.flow_id);
      return {
        view: "Flows",
        code: null,
        text: flow ? `flow ${flow.position} · ${flow.llm.title}` : "a flow not in this analysis",
        what: "this flow",
        tip: "Asked about this flow as a whole. Opens its introduction; the chat stays open.",
      };
    }
    case "pr":
      return {
        view: null,
        code: null,
        text: "Whole PR",
        what: "the whole PR",
        tip: "Asked about the whole pull request, not one place. Opens the PR context; the chat stays open.",
      };
  }
}

/** Lines of a file an anchor covers (`to` null: the whole file); null for a flow or the whole PR. */
export function linesOf(
  anchor: ChatAnchor,
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
function about(anchor: ChatAnchor, place: ChatAnchor, tour: Tour): boolean {
  switch (place.kind) {
    case "pr":
      return anchor.kind === "pr";
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
export function questionsAt(questions: readonly ChatQuestion[], place: ChatAnchor, tour: Tour): ChatThread[] {
  return threadsOf(questions).filter((t) => about(t.root.anchor, place, tour));
}

/** Ids of questions with an outcome (answer or failure) not in `seen`, oldest first. */
export function unreadOf(questions: readonly ChatQuestion[], seen: ReadonlySet<QuestionId>): QuestionId[] {
  return [...questions]
    .filter((q) => (q.state === "answered" || q.state === "failed") && !seen.has(q.id))
    .sort(asked)
    .map((q) => q.id);
}

/** Whether an answer about the anchor can become a review comment: it has lines to anchor on. */
export const hasLines = (anchor: ChatAnchor) =>
  anchor.kind === "step" || (anchor.kind === "lines" && anchor.range !== null);
