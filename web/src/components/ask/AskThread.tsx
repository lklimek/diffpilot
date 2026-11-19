import { useLayoutEffect, useRef } from "react";
import type { AskAnchor, AskQuestion, Evidence, StepId } from "../../api/generated/types";
import type { QuestionPlace } from "../questions/QuestionsBlock";
import { rangeText } from "../section/format";
import { Tooltip } from "../tooltip/Tooltip";
import { Answer } from "./AskAnswers";

/** What the thread shows: the whole conversation, or the questions one filter keeps (`"step"`: this step's). */
export type AskScope = string;

/** A filter of the conversation besides "Whole conversation" (e.g. This file, This block). */
export interface AskFilter {
  key: string;
  label: string;
  tip: string;
  /** Said when it keeps no question. */
  empty: string;
  match: (question: AskQuestion) => boolean;
}

/** Props of the Ask conversation. */
export interface AskThreadProps {
  /** The user's questions of the analysis, any order (`GET …/questions` and `question` SSE events). */
  questions: readonly AskQuestion[];
  /** The step open in the review (the "This step" filter); without it `filters` and `isCurrent` say what is here. */
  stepId?: StepId;
  scope: AskScope;
  onScope: (scope: AskScope) => void;
  /** The step of the tour a question was asked on, or null when the tour has no such step. */
  placeOf: (step: StepId) => QuestionPlace | null;
  /** Where any question was asked (its anchor link), overriding `placeOf`; null when it cannot be opened. */
  placeOfQuestion?: (question: AskQuestion) => QuestionPlace | null;
  /** The filters besides "Whole conversation"; "This step" of `stepId` without it. */
  filters?: AskFilter[];
  /** Whether a question is about what is open (its link is current; its answer can become a comment). */
  isCurrent?: (question: AskQuestion) => boolean;
  onEvidence?: (evidence: Evidence) => void;
  /** File of the open step; its evidence links are shown in bold. */
  currentFile?: string;
  /** Asks a failed question again on its step. */
  onAskAgain?: (anchor: AskAnchor, text: string) => Promise<unknown> | void;
  /** "Use as review comment" on an answer asked on the open step (anchored on that step's lines). */
  onUseAsComment?: (question: AskQuestion) => void;
}

// Within this many px of the end the reader counts as following the newest entry.
const FOLLOW_PX = 24;

const CONVERSATION = {
  key: "conversation",
  label: "Whole conversation",
  tip: "Shows every question you asked in this analysis, on any step or lines, in the order asked: your conversation about this pull request's analysis.",
};
const stepFilter = (stepId: StepId | undefined): AskFilter => ({
  key: "step",
  label: "This step",
  tip: "Shows only the questions asked on this step.",
  empty: "No question on this step yet. Whole conversation shows the questions asked on other steps.",
  match: (q) => q.step_id === stepId,
});

/**
 * The user's Ask conversation of the analysis as one thread, oldest at the top: each question with the step it was
 * asked on and its answer. Follows the newest entry unless the reader scrolled up; a new question always shows.
 */
export function AskThread({
  questions,
  stepId,
  scope,
  onScope,
  placeOf,
  placeOfQuestion,
  filters = [stepFilter(stepId)],
  isCurrent = (q) => stepId !== undefined && q.step_id === stepId,
  onEvidence,
  currentFile,
  onAskAgain,
  onUseAsComment,
}: AskThreadProps) {
  const box = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const seen = useRef<ReadonlySet<string> | null>(null);
  const filter = filters.find((f) => f.key === scope);
  const shown = [...questions]
    .filter((q) => !filter || filter.match(q))
    .sort((a, b) => Date.parse(a.created_at) - Date.parse(b.created_at));
  useLayoutEffect(() => {
    const element = box.current;
    const ids = new Set(questions.map((q) => q.id));
    const added = seen.current === null || [...ids].some((id) => !seen.current?.has(id));
    seen.current = ids;
    if (element && (added || following.current)) element.scrollTop = element.scrollHeight;
  });
  return (
    <div className="flex flex-1 flex-col gap-1.5">
      <div role="group" aria-label="Show" className="flex flex-wrap gap-1 text-[12px]">
        {[CONVERSATION, ...filters].map((s) => (
          <Tooltip key={s.key} tip={s.tip}>
            <button
              type="button"
              aria-pressed={(filter?.key ?? "conversation") === s.key}
              onClick={() => onScope(s.key)}
              className="min-h-6 cursor-pointer rounded-md border border-border bg-surface px-2 text-text hover:border-accent aria-pressed:border-accent aria-pressed:bg-accent-bg aria-pressed:font-semibold"
            >
              {s.label}
            </button>
          </Tooltip>
        ))}
      </div>
      {shown.length === 0 ? (
        <p className="text-[12.5px] text-muted">{filter?.empty ?? "No question yet."}</p>
      ) : (
        <div
          ref={box}
          role="region"
          aria-label="Scrollable conversation"
          tabIndex={0}
          onScroll={(event) => {
            const e = event.currentTarget;
            following.current = e.scrollTop + e.clientHeight >= e.scrollHeight - FOLLOW_PX;
          }}
          className="max-h-[24vh] min-h-0 flex-1 overflow-y-auto overscroll-contain [[data-sized]_&]:max-h-none"
        >
          <ol aria-label="Ask conversation" className="m-0 grid list-none gap-2.5 p-0">
            {shown.map((q) => {
              const place = filter
                ? undefined
                : placeOfQuestion
                  ? placeOfQuestion(q)
                  : q.step_id === undefined
                    ? null
                    : placeOf(q.step_id);
              return (
                <li key={q.id} className="grid min-w-0 gap-1 [overflow-wrap:anywhere]">
                  {!place && !filter && q.anchor.kind === "lines" ? (
                    <span className="text-[11.5px] text-muted">On {linesLabel(q.anchor)}</span>
                  ) : place === null ? (
                    <span className="text-[11.5px] text-muted">
                      {q.step_id === undefined
                        ? "On lines of a file (Files view)"
                        : "On a step that is not part of these flows"}
                    </span>
                  ) : (
                    place && (
                      <Tooltip
                        tip={
                          q.step_id === undefined
                            ? "Opens the lines this question was asked about."
                            : "Opens the step this question was asked on."
                        }
                        className="justify-self-start"
                      >
                        <button
                          type="button"
                          onClick={place.open}
                          aria-current={isCurrent(q) ? "step" : undefined}
                          className="min-h-6 cursor-pointer text-left text-[11.5px] text-accent underline"
                        >
                          {place.label}
                        </button>
                      </Tooltip>
                    )
                  )}
                  <p data-question className="font-semibold whitespace-pre-wrap">
                    <span className="text-muted">
                      Asked by {q.mine ? "you" : (q.asked_by ?? "unknown author")}:{" "}
                    </span>
                    {q.text}
                  </p>
                  <Answer
                    question={q}
                    onAskAgain={onAskAgain}
                    onEvidence={onEvidence}
                    currentFile={currentFile}
                    onUseAsComment={onUseAsComment && isCurrent(q) ? () => onUseAsComment(q) : undefined}
                  />
                </li>
              );
            })}
          </ol>
        </div>
      )}
    </div>
  );
}

/** `path:start–end` (or the path for a whole file) of a question asked about lines. */
function linesLabel(anchor: Extract<AskAnchor, { kind: "lines" }>): string {
  const lines = anchor.range ? `:${rangeText(anchor.range)}` : "";
  return `${anchor.path}${lines}${anchor.side === "base" ? " (old code)" : ""}`;
}
