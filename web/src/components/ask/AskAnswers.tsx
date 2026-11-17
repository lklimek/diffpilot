import type { AskQuestion } from "../../api/generated/types";
import { evidenceText } from "../section/format";
import { Tooltip } from "../tooltip/Tooltip";

/** Props of the questions asked about a step and their LLM answers. */
export interface AskAnswersProps {
  /** Questions of the current step, any order (`GET …/questions` and `question` SSE events). */
  questions: readonly AskQuestion[];
  /** Asks a failed question again; no button without it. */
  onAskAgain?: (text: string) => Promise<unknown> | void;
}

function Answer({ question, onAskAgain }: { question: AskQuestion; onAskAgain?: (text: string) => unknown }) {
  switch (question.state) {
    case "pending":
      return (
        <p role="status" className="text-muted">
          Waiting for the LLM to start on it.
        </p>
      );
    case "answering":
      return (
        <p role="status" className="text-muted">
          The LLM is working on the answer; it appears here whole when it is ready.
        </p>
      );
    case "failed":
      return (
        <p className="flex flex-wrap items-baseline gap-2 text-high">
          No answer: {question.failure ?? "the job failed."}
          {onAskAgain && (
            <button
              type="button"
              onClick={() => onAskAgain(question.text)}
              className="cursor-pointer rounded-md border border-border bg-surface px-2 py-0.5 text-xs text-text"
            >
              Ask again
            </button>
          )}
        </p>
      );
    case "answered": {
      const answer = question.answer;
      if (!answer) return <p className="text-muted">The answer is not available.</p>;
      return (
        <div className="rounded-md border-l-4 border-l-llm bg-llm-bg px-2.5 py-1.5">
          <Tooltip tip="Answer written by the LLM after reading the code; nothing was executed. LLM analysis.">
            <span
              data-llm-label
              tabIndex={0}
              className="cursor-help text-[11px] font-semibold tracking-wide text-llm uppercase"
            >
              LLM answer
            </span>
          </Tooltip>
          <p className="mt-0.5 whitespace-pre-wrap">{answer.answer}</p>
          {answer.evidence.length > 0 && (
            <p className="mt-1 text-[11.5px] text-muted">
              Lines read: {answer.evidence.map(evidenceText).join(", ")}
            </p>
          )}
        </div>
      );
    }
  }
}

/** Questions asked about a step, newest first, each with its state or its whole LLM answer (ADR-004). */
export function AskAnswers({ questions, onAskAgain }: AskAnswersProps) {
  if (questions.length === 0) {
    return <p className="text-muted">No questions asked about this step yet. Answers stay pinned here.</p>;
  }
  const newestFirst = [...questions].sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at));
  return (
    <ol aria-label="Questions asked about this step" className="m-0 grid list-none gap-2.5 p-0">
      {newestFirst.map((q) => (
        <li key={q.id} className="grid gap-1">
          <p className="font-semibold whitespace-pre-wrap">
            <span className="text-muted">You asked: </span>
            {q.text}
          </p>
          <Answer question={q} onAskAgain={onAskAgain} />
        </li>
      ))}
    </ol>
  );
}
