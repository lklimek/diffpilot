import type { AskQuestion, Evidence, StepId } from "../../api/generated/types";
import { AskAnswers } from "../ask/AskAnswers";
import { Note, QuietCard } from "../quiet/QuietCard";
import { Tooltip } from "../tooltip/Tooltip";

/** Where a question was asked: a label and the action that opens that step. */
export interface QuestionPlace {
  label: string;
  open: () => void;
}

interface QuestionsBlockProps {
  /** The analysis's questions (`GET …/questions`), in the order the steps should be listed. */
  questions: readonly AskQuestion[];
  /** The step of the tour a question was asked on, or null when the tour has no such step. */
  placeOf: (step: StepId) => QuestionPlace | null;
  onEvidence?: (evidence: Evidence) => void;
  /** Asks a failed question again on its step. */
  onAskAgain?: (step: StepId, text: string) => Promise<unknown> | void;
}

const LINK =
  "cursor-pointer rounded-md border border-border bg-surface px-2 py-0.5 text-xs text-accent hover:border-accent";

/** Every question the user asked in this review, grouped by step, each with its state or LLM answer. */
export function QuestionsBlock({ questions, placeOf, onEvidence, onAskAgain }: QuestionsBlockProps) {
  const mine = questions.filter((q) => q.mine !== false);
  const steps = [...new Set(mine.map((q) => q.step_id))];
  return (
    <QuietCard title={`Your questions and answers (${mine.length})`}>
      <Note>Only you see these questions. Answers are LLM analysis; nothing was executed.</Note>
      {mine.length === 0 ? (
        <p>You have not asked any questions in this review yet. Ask about a step in the Inspector.</p>
      ) : (
        <ul className="m-0 grid list-none gap-3 p-0">
          {steps.map((step) => {
            const place = placeOf(step);
            return (
              <li key={step} className="grid gap-1.5 border-t border-border pt-2 first:border-t-0 first:pt-0">
                {place ? (
                  <Tooltip tip="Opens the step this question was asked on.">
                    <button type="button" onClick={place.open} className={`${LINK} justify-self-start`}>
                      {place.label}
                    </button>
                  </Tooltip>
                ) : (
                  <p className="text-xs text-muted">The step of this question is not part of this tour.</p>
                )}
                <AskAnswers
                  questions={mine.filter((q) => q.step_id === step)}
                  onEvidence={onEvidence}
                  onAskAgain={onAskAgain}
                />
              </li>
            );
          })}
        </ul>
      )}
    </QuietCard>
  );
}
