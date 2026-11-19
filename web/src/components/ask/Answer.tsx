import { CodeText } from "../section/CodeText";
import type { AskQuestion, Evidence } from "../../api/generated/types";
import { busyTip } from "../button/button";
import { clockText } from "../format/time";
import { durationText, elapsedText } from "../dashboard/common/format";
import { useAction } from "../inspector/action";
import { EvidenceLinks } from "../section/EvidenceLinks";
import { UseAsCommentButton } from "../comment/UseAsCommentButton";
import { Tooltip } from "../tooltip/Tooltip";

/** Props of one question's state or answer in the Ask chat. */
export interface AnswerProps {
  question: AskQuestion;
  onEvidence?: (evidence: Evidence) => void;
  /** File of the subject; its evidence links are shown in bold. */
  currentFile?: string;
  /** Asks a failed question again; no button without it. One request at a time. */
  onAskAgain?: () => Promise<unknown> | void;
  /** "Use as review comment"; no button without it. */
  onUseAsComment?: () => void;
  /** The current time (ms): a waiting question shows the time since it was asked. */
  now?: number;
  /** An earlier question of the reader still waits: this one is queued behind it. */
  queued?: boolean;
}

function AskAgain({ onAskAgain }: Required<Pick<AnswerProps, "onAskAgain">>) {
  const action = useAction();
  return (
    <>
      <Tooltip
        tip={busyTip(
          "Asks this question again about the same place; its new answer takes the place of this one. Nothing is sent to GitHub.",
          action.busy,
        )}
      >
        <button
          type="button"
          aria-disabled={action.busy || undefined}
          onClick={() => !action.busy && void action.run(() => onAskAgain())}
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

// `1:42` within the hour, then `2 h 15 min`.
const since = (ms: number) => (ms < 3_600_000 ? elapsedText(ms) : durationText(ms));

const WRITING =
  "The LLM writes the answer; it appears whole when it is ready. A first question, or the first after the analysis changed, takes longer: the conversation is prepared first. The time shows the job is alive; it is no estimate.";

/** A question still waiting: one line with a spinner, its state and the time since it was asked (AC10). */
function Waiting({
  text,
  tip,
  question,
  now,
}: {
  text: string;
  tip: string;
  question: AskQuestion;
  now?: number;
}) {
  return (
    <Tooltip tip={tip}>
      <p tabIndex={0} className="m-0 inline-flex items-center gap-1.5 text-muted">
        <span
          aria-hidden="true"
          className="inline-block size-3 flex-none animate-spin rounded-full border-2 border-muted border-t-transparent motion-reduce:animate-none"
        />
        <span role="status">{text}</span>
        {now !== undefined && <span> · {since(now - Date.parse(question.created_at))}</span>}
      </p>
    </Tooltip>
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
  now,
  queued,
}: AnswerProps) {
  switch (question.state) {
    case "pending":
      if (question.resumes_at)
        return (
          <p role="status" className="m-0 text-muted">
            Waiting for the LLM usage limit to reset; resumes at {clockText(question.resumes_at)}.
          </p>
        );
      return queued ? (
        <Waiting
          text="Queued behind your earlier question"
          tip="Your questions are answered one at a time; this one starts when the earlier one is answered."
          question={question}
          now={now}
        />
      ) : (
        <Waiting text="Waiting to start" tip={WRITING} question={question} now={now} />
      );
    case "answering":
      return <Waiting text="Being written" tip={WRITING} question={question} now={now} />;
    case "failed":
      return (
        <p className="flex flex-wrap items-baseline gap-2 text-high">
          No answer: {question.failure ?? "the job failed."}
          {question.mine === true && onAskAgain && <AskAgain onAskAgain={onAskAgain} />}
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
