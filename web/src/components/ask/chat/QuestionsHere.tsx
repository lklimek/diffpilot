import { useId } from "react";
import type { ChatQuestion } from "../../../api/askChat";
import type { QuestionId } from "../../../api/generated/types";
import { Kbd } from "../../help/Kbd";
import { CodeText } from "../../section/CodeText";
import { Tooltip } from "../../tooltip/Tooltip";
import type { ChatThread } from "./model";

/** Props of the Inspector's read-only copy of the chat. */
export interface QuestionsHereProps {
  /** The threads about this place (`questionsAt`). */
  threads: ChatThread[];
  /** "this step", "these lines", "this file", "this flow", "the whole PR". */
  what: string;
  /** Opens the chat at a thread. */
  onOpenInChat: (id: QuestionId) => void;
  /** Opens the chat about this place; nothing is sent. */
  onAsk: () => void;
}

const BUTTON =
  "min-h-6 cursor-pointer rounded-md border border-border bg-surface px-2 text-[12px] text-text hover:border-accent";
const STATE: Record<ChatQuestion["state"], string> = {
  pending: "waiting",
  answering: "being answered",
  answered: "answered",
  failed: "no answer",
};

function Outcome({ question }: { question: ChatQuestion }) {
  if (question.state === "answered" && question.answer) {
    return (
      <p className="m-0 mb-1 line-clamp-3 border-l-[3px] border-l-llm pl-1.5 whitespace-pre-wrap">
        <span className="text-[10.5px] font-semibold tracking-wide text-llm uppercase">LLM answer</span>{" "}
        <CodeText text={question.answer.answer.split("\n").slice(0, 3).join("\n")} />
      </p>
    );
  }
  return (
    <p className="m-0 mb-0.5 text-muted">
      {question.state === "failed"
        ? `No answer: ${question.failure ?? "the job failed."}`
        : question.state === "answering"
          ? "The LLM is working on the answer."
          : "Waiting for the LLM."}
    </p>
  );
}

/**
 * "Your questions about this step (n)" (proposal AC11): a read-only copy of the chat's threads about the place, each
 * with its state or first answer lines, its follow-ups and Open in chat. Typing happens only in the chat.
 */
export function QuestionsHere({ threads, what, onOpenInChat, onAsk }: QuestionsHereProps) {
  const id = useId();
  const title = `Your questions about ${what} (${threads.length})`;
  return (
    <section aria-labelledby={id} className="grid gap-1.5 border-t border-border px-3 py-2 text-[12.5px]">
      <h3 id={id} className="m-0 text-[10.5px] font-bold tracking-wide text-muted uppercase">
        {title}
      </h3>
      <p className="m-0 text-[11.5px] text-muted">
        A copy from your Ask chat; read the whole thread and reply there.
      </p>
      <p className="m-0">
        <Tooltip tip={`Opens your Ask chat about ${what}. Nothing is sent until you press Ask.`}>
          <button type="button" onClick={onAsk} className={BUTTON}>
            Ask about {what} <Kbd decorative>a</Kbd>
          </button>
        </Tooltip>
      </p>
      {threads.length === 0 ? (
        <p className="m-0 text-muted">No questions about {what} yet.</p>
      ) : (
        <ul className="m-0 grid list-none gap-1.5 p-0">
          {threads.map(({ root, followUps }) => {
            const latest = followUps.at(-1);
            return (
              <li key={root.id} className="rounded-md border border-border bg-surface-2 px-2 py-1.5">
                <p className="m-0 mb-0.5 line-clamp-2 font-semibold">{root.text}</p>
                <Outcome question={root} />
                {latest && (
                  <p className="m-0 mb-0.5 text-muted">
                    {followUps.length} follow-up{followUps.length > 1 ? "s" : ""} · latest{" "}
                    {STATE[latest.state]}
                  </p>
                )}
                <Tooltip tip="Opens the chat at this question, with its answer and follow-ups.">
                  <button type="button" onClick={() => onOpenInChat(root.id)} className={BUTTON}>
                    Open in chat
                  </button>
                </Tooltip>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
