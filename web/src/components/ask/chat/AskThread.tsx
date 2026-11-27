import { useEffect, useId, useRef, useState, type KeyboardEvent, type SetStateAction } from "react";
import type { AskAnchor, AskQuestion } from "../../../api/generated/types";
import { busyTip } from "../../button/button";
import { clockText, dateTimeText } from "../../format/time";
import { growField, useAction } from "../../inspector/action";
import { Tooltip } from "../../tooltip/Tooltip";
import { Answer } from "../Answer";
import type { AskChatProps } from "./AskChat";
import { hasLines, remarkLines, remarkQuote, type ChatThread } from "./model";

// The server's message says what to do; a hint is added only to a retryable failure whose message has none.
const RETRY_LATER = new Set(["unavailable", "rate_limited"]);
export function notSent(message: string, code: string | null): string {
  if (/try again|retry/i.test(message)) return `Not sent: ${message}`;
  if (code !== null && RETRY_LATER.has(code)) return `Not sent: ${message} Try again in a moment.`;
  return code === null || code === "internal" ? `Not sent: ${message} Try again.` : `Not sent: ${message}`;
}

/** Why a question was not sent; a lost sign-in gets the "Sign in again" link (the text is kept). */
export function NotSent({ id, text, signIn }: { id?: string; text: string; signIn?: string }) {
  return (
    <p
      id={id}
      role="alert"
      className="m-0 mt-1 flex flex-wrap items-baseline gap-x-2 text-[12px] text-high before:content-['⚠']"
    >
      {text}
      {signIn && (
        <Tooltip tip="Opens the GitHub sign-in; your text stays here and nothing is sent.">
          <a href={signIn} className="font-semibold text-accent underline">
            Sign in again
          </a>
        </Tooltip>
      )}
    </p>
  );
}

const BUTTON =
  "min-h-6 cursor-pointer rounded-md border border-border bg-surface px-2 text-[12px] text-text hover:border-accent aria-disabled:cursor-wait aria-disabled:opacity-60";
export const sends = (event: KeyboardEvent<HTMLTextAreaElement>) =>
  event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing;

function Reply({
  root,
  number,
  draft,
  onDraft,
  onReply,
  onClose,
  signInHref,
}: {
  root: AskQuestion;
  number: string;
  draft: string;
  onDraft: (text: SetStateAction<string>) => void;
  onReply: AskChatProps["onReply"];
  onClose: () => void;
  signInHref: string;
}) {
  const id = useId();
  const action = useAction();
  const field = useRef<HTMLTextAreaElement>(null);
  const [empty, setEmpty] = useState(false);
  useEffect(() => {
    field.current?.focus();
    growField(field.current);
  }, []);
  const send = async () => {
    if (!draft.trim()) return setEmpty(true);
    if (await action.run(() => onReply(root, draft.trim()))) {
      onDraft((current) => (current === draft ? "" : current));
      onClose();
    }
  };
  const error = empty ? "Write a follow-up first." : action.error && notSent(action.error, action.code);
  return (
    <form
      data-reply
      noValidate
      onSubmit={(event) => {
        event.preventDefault();
        void send();
      }}
      className="ml-3.5 grid gap-1 border-l-2 border-accent py-0.5 pl-2.5"
    >
      <label htmlFor={id} className="text-[12px] font-semibold">
        Follow-up to {number} <span className="font-normal text-muted">— the LLM gets this thread</span>
      </label>
      <div className="flex items-end gap-1.5">
        <textarea
          id={id}
          ref={field}
          rows={1}
          value={draft}
          placeholder="Ask a follow-up…"
          aria-describedby={error ? `${id}-e` : undefined}
          onChange={(event) => {
            onDraft(event.target.value);
            setEmpty(false);
            growField(event.target);
          }}
          onKeyDown={(event) => {
            if (!sends(event)) return;
            event.preventDefault();
            void send();
          }}
          className="min-h-8 flex-1 resize-none rounded-md border border-border bg-surface px-2 py-1 text-[13px]"
        />
        <Tooltip
          tip={busyTip("Sends the follow-up. It keeps this thread’s subject and context.", action.busy)}
        >
          <button type="submit" aria-disabled={action.busy || undefined} className={BUTTON}>
            Send follow-up
          </button>
        </Tooltip>
        <Tooltip tip="Closes the field (Esc); your text is kept.">
          <button type="button" onClick={onClose} className={BUTTON}>
            Cancel
          </button>
        </Tooltip>
      </div>
      {error && (
        <NotSent
          id={`${id}-e`}
          text={error}
          signIn={action.code === "unauthenticated" && !empty ? signInHref : undefined}
        />
      )}
    </form>
  );
}

const OUTCOME = { answered: "answered", failed: "⚠ failed" };
const timeOf = (iso: string) => clockText(iso, new Date(iso));
const settled = (q: AskQuestion) => q.state === "answered" || q.state === "failed";

function timestampTip(question: AskQuestion, prefix = ""): string {
  const time = prefix + dateTimeText(question.created_at);
  if (question.state !== "answered" || !question.answered_at) return time;
  const ms = Date.parse(question.answered_at) - Date.parse(question.created_at);
  return Number.isFinite(ms) && ms >= 0 ? `${time} · Took ${Math.round(ms / 1000)}s` : time;
}

function QuestionState({
  question,
  waitsFor,
  answeringSince,
  live,
}: {
  question: AskQuestion;
  waitsFor?: string;
  answeringSince?: number;
  live: boolean;
}) {
  const { state, resumes_at: resumes } = question;
  if (state === "answered" || state === "failed") {
    return <span className={state === "failed" ? "text-high" : undefined}>{OUTCOME[state]}</span>;
  }
  if (state === "answering") {
    return (
      <span role={live ? "status" : undefined} className="text-accent">
        ◔ answering
        {answeringSince !== undefined && ` since ${timeOf(new Date(answeringSince).toISOString())}`}
      </span>
    );
  }
  return (
    <span role={live ? "status" : undefined}>
      ◌{" "}
      {resumes
        ? `usage limit · resumes at ${clockText(resumes)}`
        : waitsFor
          ? `waits for ${waitsFor}`
          : "waiting"}
    </span>
  );
}

interface ThreadProps
  extends
    ChatThread,
    Pick<
      AskChatProps,
      "describe" | "onGoto" | "onAskAgain" | "onEvidence" | "onUseAsComment" | "onReply" | "signInHref"
    > {
  number: string;
  fresh: boolean;
  expanded: boolean;
  onToggle: () => void;
  register: (element: HTMLLIElement | null) => void;
  waitsFor: (q: AskQuestion) => string | undefined;
  answeringSince: (q: AskQuestion) => number | undefined;
  replyOpen: boolean;
  replyDraft: string;
  onReplyDraft: (text: SetStateAction<string>) => void;
  onOpenReply: () => void;
  onCloseReply: () => void;
}

const QUESTION = "m-0 text-[13px] whitespace-pre-wrap [overflow-wrap:anywhere]";

/** One numbered question, its answer and its follow-ups. */
export function AskThread(props: ThreadProps) {
  const {
    root,
    followUps,
    number,
    expanded,
    fresh,
    describe,
    waitsFor,
    answeringSince,
    register,
    onEvidence,
    onAskAgain,
    onUseAsComment,
  } = props;
  const answerActions = { onEvidence, onAskAgain, onUseAsComment };
  const body = useId();
  const subject = describe(root.anchor);
  const remark = remarkLines(root.text, root.anchor);
  const summary = root.text.replace(remarkQuote, "").trim() || root.text;
  const questions = [root, ...followUps];
  const active =
    questions.findLast((q) => q.state === "answering") ??
    questions.findLast((q) => q.state === "pending") ??
    questions.findLast((q) => q.state === "failed") ??
    root;
  const chip = remark ? `Remark · ${remark}` : subject.text;
  const state = (q: AskQuestion, live = false) => (
    <QuestionState question={q} waitsFor={waitsFor(q)} answeringSince={answeringSince(q)} live={live} />
  );
  return (
    <li
      ref={register}
      data-thread={root.id}
      tabIndex={-1}
      aria-label={`Question: ${summary}`}
      className="min-w-0 scroll-m-2 rounded-lg border border-border focus-visible:outline-3 focus-visible:outline-focus"
    >
      <div className="flex min-w-0 items-center gap-x-1.5 rounded-t-lg bg-surface-2 px-1 py-1 text-[11.5px] text-muted">
        <Tooltip
          tip={expanded ? "Collapse this question and its replies" : "Show this question and its replies"}
        >
          <button
            type="button"
            aria-label={`${expanded ? "Hide" : "Show"} ${number}`}
            aria-expanded={expanded}
            aria-controls={body}
            onClick={props.onToggle}
            className="size-6 shrink-0 cursor-pointer rounded hover:bg-surface"
          >
            <span aria-hidden="true">{expanded ? "▾" : "▸"}</span>
          </button>
        </Tooltip>
        <span className="shrink-0 font-bold text-text">{number}</span>
        <Tooltip
          tip={subject.fileLabel ? `${subject.fileLabel} · ${subject.tip}` : subject.tip}
          className="min-w-0"
        >
          <button
            type="button"
            aria-label={subject.fileLabel ? `${chip} · ${subject.fileLabel}` : undefined}
            onClick={() => props.onGoto(root.anchor)}
            className="block min-h-6 max-w-full cursor-pointer truncate rounded-[10px] border border-border bg-surface px-1.5 text-left text-accent hover:border-accent hover:underline"
          >
            {chip}
          </button>
        </Tooltip>
        <span className="min-w-0 flex-1 truncate">{!expanded && `“${summary}”`}</span>
        {fresh && <span className="shrink-0 font-semibold text-text">new</span>}
        <span className="shrink text-right">{state(active, true)}</span>
        <time dateTime={root.created_at} title={timestampTip(root, "Asked by you · ")} className="shrink-0">
          {timeOf(root.created_at)}
        </time>
      </div>
      <div id={body} hidden={!expanded}>
        {expanded && (
          <div className="grid min-w-0 gap-1.5 p-2">
            <p data-question className={QUESTION}>
              {root.text}
            </p>
            {settled(root) && <ChatAnswer {...answerActions} question={root} anchor={root.anchor} />}
            {followUps.length > 0 && (
              <ol
                aria-label="Follow-ups"
                className="mt-1 ml-2 grid list-none gap-2.5 border-l-2 border-border p-0 pl-2.5"
              >
                {followUps.map((q) => (
                  <li key={q.id} className="grid min-w-0 gap-1">
                    <div className="flex flex-wrap items-baseline gap-x-2 text-[11.5px] text-muted">
                      <span>
                        ↳ Follow-up ·{" "}
                        <time dateTime={q.created_at} title={timestampTip(q)}>
                          {timeOf(q.created_at)}
                        </time>
                      </span>
                      {state(q)}
                    </div>
                    <p data-question className={QUESTION}>
                      {q.text}
                    </p>
                    {settled(q) && <ChatAnswer {...answerActions} question={q} anchor={root.anchor} />}
                  </li>
                ))}
              </ol>
            )}
            {props.replyOpen ? (
              <Reply
                root={root}
                number={number}
                draft={props.replyDraft}
                onDraft={props.onReplyDraft}
                onReply={props.onReply}
                onClose={props.onCloseReply}
                signInHref={props.signInHref}
              />
            ) : (
              <Tooltip
                tip="Continues this thread with the same subject and its earlier answers."
                className="ml-2 justify-self-start"
              >
                <button type="button" data-reply-button onClick={props.onOpenReply} className={BUTTON}>
                  <span aria-hidden="true">↳ </span>Follow up on {number}
                </button>
              </Tooltip>
            )}
          </div>
        )}
      </div>
    </li>
  );
}
/** The state or answer of a question; follow-ups take the thread's subject for "Use as review comment". */
function ChatAnswer({
  question,
  anchor,
  onAskAgain,
  onEvidence,
  onUseAsComment,
}: { question: AskQuestion; anchor: AskAnchor } & Pick<
  AskChatProps,
  "onAskAgain" | "onEvidence" | "onUseAsComment"
>) {
  return (
    <div data-ask-outcome={question.id} className="min-w-0 text-[13px]">
      <Answer
        question={question}
        onAskAgain={() => onAskAgain(question)}
        onEvidence={onEvidence}
        onUseAsComment={onUseAsComment && hasLines(anchor) ? () => onUseAsComment(question) : undefined}
      />
    </div>
  );
}
