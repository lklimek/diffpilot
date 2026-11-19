import { CodeText } from "../section/CodeText";
import type { AskQuestion, Evidence, StepId } from "../../api/generated/types";
import { busyTip } from "../button/button";
import { clockText } from "../format/time";
import { useAction } from "../inspector/action";
import { EvidenceLinks } from "../section/EvidenceLinks";
import { UseAsCommentButton } from "../comment/UseAsCommentButton";
import { Tooltip } from "../tooltip/Tooltip";

/** Props of the questions asked about a step and their LLM answers. */
export interface AskAnswersProps {
  /** Questions of the current step, any order (`GET …/questions` and `question` SSE events). */
  questions: readonly AskQuestion[];
  onEvidence?: (evidence: Evidence) => void;
  /** File of the step asked about; its evidence links are shown in bold. */
  currentFile?: string;
  /** Asks a failed question again on its step; no button without it. One request at a time per question. */
  onAskAgain?: (step: StepId, text: string) => Promise<unknown> | void;
}

function AskAgain({
  question,
  onAskAgain,
}: { question: AskQuestion } & Required<Pick<AskAnswersProps, "onAskAgain">>) {
  const action = useAction();
  const step = question.step_id;
  if (step === undefined) return null;
  return (
    <>
      <Tooltip
        tip={busyTip(
          "Asks the same question about this step again; the answer appears here. Nothing is sent to GitHub.",
          action.busy,
        )}
      >
        <button
          type="button"
          aria-disabled={action.busy || undefined}
          onClick={() => !action.busy && void action.run(() => onAskAgain(step, question.text))}
          className="cursor-pointer rounded-md border border-border bg-surface px-2 py-0.5 text-xs text-text aria-disabled:cursor-wait aria-disabled:opacity-60"
        >
          {action.busy ? "Asking…" : "Ask again"}
        </button>
      </Tooltip>
      {action.error && (
        <span role="alert" className="basis-full">
          Not asked again: {action.error}
        </span>
      )}
    </>
  );
}

// A list line as the LLM typed it: "- ", "* ", "• ", "1. " or "1) " (the Ask plain-text check allows them).
const LIST_LINE = /^\s*(?:[-*•]|\d+[.)])\s+(?=\S)/;

/**
 * Answer text: line breaks kept, backtick and fenced code drawn as code; list lines (outside fences) as typed, one
 * block each with a hanging indent so a wrapped line stays under its text.
 */
export function AnswerText({ text }: { text: string }) {
  const parts: ({ list: string; marker: number } | { text: string[] })[] = [];
  let fence = false;
  for (const line of text.split("\n")) {
    const list = !fence && LIST_LINE.exec(line);
    if (line.startsWith("```")) fence = !fence;
    const last = parts.at(-1);
    if (list) parts.push({ list: line, marker: list[0].length });
    else if (last && "text" in last) last.text.push(line);
    else parts.push({ text: [line] });
  }
  if (!parts.some((p) => "list" in p)) return <CodeText text={text} />;
  return (
    <>
      {parts.map((part, i) => {
        if ("text" in part)
          return (
            <span key={i} className="block min-h-[1lh]">
              <CodeText text={part.text.join("\n")} />
            </span>
          );
        const indent = `${part.marker}ch`;
        return (
          <span
            key={i}
            data-list-line
            className="block"
            style={{ paddingLeft: indent, textIndent: `-${indent}` }}
          >
            <CodeText text={part.list} />
          </span>
        );
      })}
    </>
  );
}

/** The state of a question, or its whole LLM answer with evidence. */
export function Answer({
  question,
  onAskAgain,
  onEvidence,
  currentFile,
  onUseAsComment,
}: { question: AskQuestion; onUseAsComment?: () => void } & Pick<
  AskAnswersProps,
  "onAskAgain" | "onEvidence" | "currentFile"
>) {
  switch (question.state) {
    case "pending":
      return (
        <p role="status" className="text-muted">
          {question.resumes_at
            ? `Waiting for the LLM usage limit to reset; the question resumes automatically at ${clockText(question.resumes_at)}.`
            : "Waiting for the LLM to start on it."}
        </p>
      );
    case "answering":
      return (
        <p role="status" className="text-muted">
          The LLM is working on the answer; it appears here whole when it is ready. A first question, or the
          first after the analysis changed, takes longer: the conversation is prepared first.
        </p>
      );
    case "failed":
      return (
        <p className="flex flex-wrap items-baseline gap-2 text-high">
          No answer: {question.failure ?? "the job failed."}
          {question.mine === true && onAskAgain && <AskAgain question={question} onAskAgain={onAskAgain} />}
        </p>
      );
    case "answered": {
      const answer = question.answer;
      if (!answer) return <p className="text-muted">The answer is not available.</p>;
      return (
        <div className="rounded-md border-l-4 border-l-llm bg-llm-bg px-2.5 py-1.5">
          <Tooltip tip="Answer written by the LLM after reading the code; the agent was told not to run code. LLM analysis.">
            <span
              data-llm-label
              tabIndex={0}
              className="cursor-help text-[11px] font-semibold tracking-wide text-llm uppercase"
            >
              LLM answer
            </span>
          </Tooltip>
          <div className="mt-0.5 min-w-0 whitespace-pre-wrap [overflow-wrap:anywhere] [&_pre]:whitespace-pre-wrap">
            <AnswerText text={answer.answer} />
          </div>
          {answer.evidence.length > 0 && (
            <div className="mt-1 text-[11.5px] text-muted">
              <EvidenceLinks evidence={answer.evidence} onEvidence={onEvidence} currentFile={currentFile} />
            </div>
          )}
          {onUseAsComment && (
            <div className="mt-1">
              <UseAsCommentButton onClick={onUseAsComment} what="this answer" />
            </div>
          )}
        </div>
      );
    }
  }
}

/** Questions asked about a step, newest first, each with its state or its whole LLM answer (ADR-004). */
export function AskAnswers({ questions, onAskAgain, onEvidence, currentFile }: AskAnswersProps) {
  if (questions.length === 0) return null;
  const newestFirst = [...questions].sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at));
  return (
    <ol aria-label="Questions asked about this step" className="m-0 grid list-none gap-2.5 p-0">
      {newestFirst.map((q) => (
        <li key={q.id} className="grid min-w-0 gap-1 [overflow-wrap:anywhere]">
          <p className="font-semibold whitespace-pre-wrap">
            <span className="text-muted">Asked by {q.mine ? "you" : (q.asked_by ?? "unknown author")}: </span>
            {q.text}
          </p>
          <Answer question={q} onAskAgain={onAskAgain} onEvidence={onEvidence} currentFile={currentFile} />
        </li>
      ))}
    </ol>
  );
}
