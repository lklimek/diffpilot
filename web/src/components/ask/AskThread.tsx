import { useLayoutEffect, useRef } from "react";
import type { AskQuestion, Evidence, StepId } from "../../api/generated/types";
import type { QuestionPlace } from "../questions/QuestionsBlock";
import { Tooltip } from "../tooltip/Tooltip";
import { Answer } from "./AskAnswers";

/** What the thread shows: the whole conversation of the analysis, or only the current step's questions. */
export type AskScope = "conversation" | "step";

/** Props of the Ask conversation. */
export interface AskThreadProps {
  /** The user's questions of the analysis, any order (`GET …/questions` and `question` SSE events). */
  questions: readonly AskQuestion[];
  /** The step open in the review. */
  stepId: StepId;
  scope: AskScope;
  onScope: (scope: AskScope) => void;
  /** The step of the tour a question was asked on, or null when the tour has no such step. */
  placeOf: (step: StepId) => QuestionPlace | null;
  onEvidence?: (evidence: Evidence) => void;
  /** File of the open step; its evidence links are shown in bold. */
  currentFile?: string;
  /** Asks a failed question again on its step. */
  onAskAgain?: (step: StepId, text: string) => Promise<unknown> | void;
  /** "Use as review comment" on an answer asked on the open step (anchored on that step's lines). */
  onUseAsComment?: (question: AskQuestion) => void;
}

// Within this many px of the end the reader counts as following the newest entry.
const FOLLOW_PX = 24;

const SCOPES: { scope: AskScope; label: string; tip: string }[] = [
  {
    scope: "conversation",
    label: "Whole conversation",
    tip: "Shows every question you asked in this analysis, on any step, in the order asked: your conversation about this pull request's analysis.",
  },
  { scope: "step", label: "This step", tip: "Shows only the questions asked on this step." },
];

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
  onEvidence,
  currentFile,
  onAskAgain,
  onUseAsComment,
}: AskThreadProps) {
  const box = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const seen = useRef<ReadonlySet<string> | null>(null);
  const shown = [...questions]
    .filter((q) => scope === "conversation" || q.step_id === stepId)
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
        {SCOPES.map((s) => (
          <Tooltip key={s.scope} tip={s.tip}>
            <button
              type="button"
              aria-pressed={scope === s.scope}
              onClick={() => onScope(s.scope)}
              className="min-h-6 cursor-pointer rounded-md border border-border bg-surface px-2 text-text hover:border-accent aria-pressed:border-accent aria-pressed:bg-accent-bg aria-pressed:font-semibold"
            >
              {s.label}
            </button>
          </Tooltip>
        ))}
      </div>
      {shown.length === 0 ? (
        <p className="text-[12.5px] text-muted">
          No question on this step yet. Whole conversation shows the questions asked on other steps.
        </p>
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
              const place =
                scope === "conversation" ? (q.step_id === undefined ? null : placeOf(q.step_id)) : undefined;
              return (
                <li key={q.id} className="grid min-w-0 gap-1 [overflow-wrap:anywhere]">
                  {place === null ? (
                    <span className="text-[11.5px] text-muted">On a step that is not part of this tour</span>
                  ) : (
                    place && (
                      <Tooltip
                        tip="Opens the step this question was asked on."
                        className="justify-self-start"
                      >
                        <button
                          type="button"
                          onClick={place.open}
                          aria-current={q.step_id === stepId ? "step" : undefined}
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
                    onUseAsComment={
                      onUseAsComment && q.step_id === stepId ? () => onUseAsComment(q) : undefined
                    }
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
