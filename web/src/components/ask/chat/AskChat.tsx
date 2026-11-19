import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
  type Ref,
} from "react";
import type { ChatAnchor, ChatQuestion } from "../../../api/askChat";
import type { AskQuestion, Evidence, QuestionId, Tour } from "../../../api/generated/types";
import { HEAD_BUTTON, busyTip } from "../../button/button";
import { clockText } from "../../format/time";
import { Kbd } from "../../help/Kbd";
import { assignRef, growField, useAction } from "../../inspector/action";
import { Tooltip } from "../../tooltip/Tooltip";
import { Answer } from "../AskAnswers";
import { hasLines, subjectOf, type ChatThread } from "./model";

/** The chat's remembered size: null = the default; `full` = the whole height above the dock. */
export interface ChatSize {
  width: number | null;
  height: number | null;
  full: boolean;
}

/** Where the panel is drawn (px from the window's right and bottom edge), its size and the size limits. */
export interface ChatGeometry {
  right: number;
  bottom: number;
  width: number;
  height: number;
  minWidth: number;
  maxWidth: number;
  minHeight: number;
  maxHeight: number;
  /** A narrow window: a full-width bottom sheet. */
  narrow?: boolean;
}

/** Props of the floating Ask chat. */
export interface AskChatProps {
  tour: Tour;
  /** The user's threads of the analysis, oldest first (`threadsOf`). */
  threads: ChatThread[];
  /** What the next question is about. */
  subject: ChatAnchor;
  /** The subject follows the reader's place (× asks about the whole PR instead). */
  following: boolean;
  /** "this step", "these lines"…: where the reader is, offered while the subject is the whole PR by choice. */
  placeWhat: string | null;
  draft: string;
  onDraft: (text: string) => void;
  /** Sends a new question about `subject`; a rejection keeps the text and shows its message. */
  onSend: (text: string) => Promise<unknown> | void;
  /** Sends a follow-up in the thread of `root` (same subject; the LLM gets the thread). */
  onReply: (root: ChatQuestion, text: string) => Promise<unknown> | void;
  /** Asks a question without an answer again, in its thread. */
  onAskAgain: (question: ChatQuestion) => Promise<unknown> | void;
  onUnanchor: () => void;
  onUsePlace: () => void;
  /** Opens what a thread is about (its subject chip); the chat stays open. */
  onGoto: (anchor: ChatAnchor) => void;
  onEvidence?: (evidence: Evidence) => void;
  /** "Use as review comment" on an answer about lines; absent while commenting is unavailable. */
  onUseAsComment?: (question: ChatQuestion) => void;
  /** Hides the chat (Esc); its drafts and answers are kept. */
  onHide: () => void;
  size: ChatSize;
  geometry: ChatGeometry;
  onSize: (size: ChatSize) => void;
  /** The privacy line under the title. */
  about: string;
  /** Questions whose outcome the reader has not seen: their threads are marked new. */
  unread: ReadonlySet<QuestionId>;
  /** A thread to scroll to and focus ("Open in chat"); `nonce` repeats it. */
  focusThread: { id: QuestionId; nonce: number } | null;
  inputRef?: Ref<HTMLTextAreaElement>;
}

const BUTTON =
  "min-h-6 cursor-pointer rounded-md border border-border bg-surface px-2 text-[12px] text-text hover:border-accent aria-disabled:cursor-wait aria-disabled:opacity-60";
const PRIMARY =
  "min-h-8 cursor-pointer rounded-md border border-accent bg-accent px-3 text-[13px] font-semibold text-on-accent aria-disabled:cursor-wait aria-disabled:opacity-60";
// Within this many px of the end the reader counts as following the newest entry.
const FOLLOW_PX = 24;
const KEY_PX = 20;

const sends = (event: KeyboardEvent<HTMLTextAreaElement>) =>
  event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing;

/** A subject as chip text: view, code, words. */
function SubjectText({ anchor, tour }: { anchor: ChatAnchor; tour: Tour }) {
  const s = subjectOf(anchor, tour);
  return (
    <>
      {s.view && <span className="text-[10px] font-bold tracking-wide text-muted uppercase">{s.view}</span>}{" "}
      {s.code && <code className="text-[11px] text-text [overflow-wrap:anywhere]">{s.code}</code>}{" "}
      {s.text && <span>{s.text}</span>}
    </>
  );
}

function Question({
  question,
  root,
  tour,
  onGoto,
}: { question: ChatQuestion; root: boolean } & Pick<AskChatProps, "tour" | "onGoto">) {
  return (
    <div className="flex min-w-0 flex-col items-end gap-0.5">
      <div className="flex max-w-full flex-wrap items-center justify-end gap-1.5 text-[11.5px] text-muted">
        {root ? (
          <Tooltip tip={subjectOf(question.anchor, tour).tip}>
            <button
              type="button"
              onClick={() => onGoto(question.anchor)}
              className="inline-flex min-h-6 max-w-full cursor-pointer flex-wrap items-baseline gap-1 rounded-[10px] border border-border bg-surface px-2 text-left text-[11.5px] text-accent hover:border-accent hover:underline"
            >
              <SubjectText anchor={question.anchor} tour={tour} />
            </button>
          </Tooltip>
        ) : (
          <Tooltip tip="Asked in this thread: the LLM had its earlier questions and answers.">
            <span tabIndex={0}>Follow-up</span>
          </Tooltip>
        )}
        <span>Asked by you · {clockText(question.created_at)}</span>
      </div>
      <p
        data-question
        className="m-0 max-w-[90%] rounded-[10px_10px_2px_10px] border border-accent bg-accent-bg px-2.5 py-1.5 text-[13px] whitespace-pre-wrap [overflow-wrap:anywhere]"
      >
        {question.text}
      </p>
    </div>
  );
}

function Reply({
  root,
  draft,
  onDraft,
  onReply,
  onClose,
}: {
  root: ChatQuestion;
  draft: string;
  onDraft: (text: string) => void;
  onReply: AskChatProps["onReply"];
  onClose: (sent: boolean) => void;
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
      onDraft("");
      onClose(true);
    }
  };
  const error = empty ? "Write a follow-up first." : action.error && `Not sent: ${action.error} Try again.`;
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
        Follow-up in this thread{" "}
        <span className="font-normal text-muted">· the LLM gets this thread’s questions and answers</span>
      </label>
      <div className="flex items-end gap-1.5">
        <textarea
          id={id}
          ref={field}
          rows={1}
          value={draft}
          placeholder="Ask a follow-up…"
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
            Send
          </button>
        </Tooltip>
        <Tooltip tip="Closes the field (Esc); your text is kept.">
          <button type="button" onClick={() => onClose(false)} className={BUTTON}>
            Cancel
          </button>
        </Tooltip>
      </div>
      {error && (
        <p role="alert" className="m-0 text-[12px] text-high">
          {error}
        </p>
      )}
    </form>
  );
}

/** One separator of the panel's size: drag, arrow keys, Home / End, Enter or double-click resets. */
function SizeHandle({
  dimension,
  geometry,
  size,
  onSize,
}: { dimension: "width" | "height" } & Pick<AskChatProps, "geometry" | "size" | "onSize">) {
  const start = useRef<{ x: number; y: number; value: number } | null>(null);
  const height = dimension === "height";
  const [min, max, now] = height
    ? [geometry.minHeight, geometry.maxHeight, geometry.height]
    : [geometry.minWidth, geometry.maxWidth, geometry.width];
  const set = (value: number | null) =>
    onSize(
      height
        ? { ...size, height: value === null ? null : Math.round(value), full: false }
        : { ...size, width: value === null ? null : Math.round(value) },
    );
  return (
    <div
      role="separator"
      aria-orientation={height ? "horizontal" : "vertical"}
      tabIndex={0}
      aria-label={
        height
          ? "Chat height: drag, or use the up and down arrow keys; Home and End go to the limits; Enter or double-click resets"
          : "Chat width: drag, or use the left and right arrow keys; Home and End go to the limits; Enter or double-click resets"
      }
      aria-valuemin={Math.round(min)}
      aria-valuemax={Math.round(max)}
      aria-valuenow={Math.round(now)}
      onPointerDown={(event: PointerEvent<HTMLDivElement>) => {
        start.current = { x: event.clientX, y: event.clientY, value: now };
        event.currentTarget.setPointerCapture?.(event.pointerId);
        event.preventDefault();
      }}
      onPointerMove={(event: PointerEvent<HTMLDivElement>) => {
        const from = start.current;
        if (!from) return;
        set(from.value + (height ? from.y - event.clientY : from.x - event.clientX));
      }}
      onPointerUp={() => (start.current = null)}
      onPointerCancel={() => (start.current = null)}
      onDoubleClick={() => set(null)}
      onKeyDown={(event) => {
        const grow = height ? "ArrowUp" : "ArrowLeft";
        const shrink = height ? "ArrowDown" : "ArrowRight";
        const next =
          event.key === grow
            ? now + KEY_PX
            : event.key === shrink
              ? now - KEY_PX
              : event.key === "Home"
                ? min
                : event.key === "End"
                  ? max
                  : event.key === "Enter"
                    ? null
                    : undefined;
        if (next === undefined) return;
        event.preventDefault();
        set(next === null ? null : Math.min(max, Math.max(min, next)));
      }}
      className={`absolute z-[2] touch-none rounded bg-transparent hover:bg-accent focus-visible:bg-accent ${
        height ? "inset-x-3.5 -top-1 h-2 cursor-ns-resize" : "inset-y-3.5 -left-1 w-2 cursor-ew-resize"
      }`}
    />
  );
}

/**
 * The floating Ask chat (proposal AC2–AC10): one private conversation per analysis and user, threads oldest first
 * with their subject chips and follow-ups, the composer whose subject follows the reader, Full height, Hide and
 * resizing. Non-modal: focus is not trapped.
 */
export function AskChat(props: AskChatProps) {
  const { tour, threads, subject, following, placeWhat, draft, onDraft, geometry, size, onSize } = props;
  const titleId = useId();
  const aboutId = useId();
  const fieldId = useId();
  const list = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLTextAreaElement>(null);
  const following_ = useRef(true);
  const count = useRef(0);
  const action = useAction();
  const [empty, setEmpty] = useState(false);
  const [replyFor, setReplyFor] = useState<QuestionId | null>(null);
  const [replyDrafts, setReplyDrafts] = useState<Record<QuestionId, string>>({});
  const replyButtons = useRef(new Map<QuestionId, HTMLButtonElement>());
  const threadItems = useRef(new Map<QuestionId, HTMLLIElement>());
  const s = subjectOf(subject, tour);
  const total = threads.reduce((n, t) => n + 1 + t.followUps.length, 0);

  // Follows the newest entry unless the reader scrolled up; a question just sent always shows.
  useLayoutEffect(() => {
    const element = list.current;
    const added = total > count.current;
    count.current = total;
    if (element && (added || following_.current)) element.scrollTop = element.scrollHeight;
  });
  const focusId = props.focusThread?.id;
  const focusNonce = props.focusThread?.nonce;
  useEffect(() => {
    if (!focusId) return;
    const item = threadItems.current.get(focusId);
    item?.scrollIntoView?.({ block: "start" });
    item?.focus({ preventScroll: true });
  }, [focusId, focusNonce]);

  const send = async () => {
    if (!draft.trim()) {
      setEmpty(true);
      field.current?.focus();
      return;
    }
    if (await action.run(() => props.onSend(draft.trim()))) {
      onDraft("");
      requestAnimationFrame(() => growField(field.current));
    }
  };
  const closeReply = (id: QuestionId) => {
    setReplyFor(null);
    requestAnimationFrame(() => replyButtons.current.get(id)?.focus());
  };
  const error = empty ? "Write a question first." : action.error && `Not sent: ${action.error} Try again.`;

  return (
    <section
      role="dialog"
      aria-modal="false"
      aria-labelledby={titleId}
      aria-describedby={aboutId}
      data-ask-chat
      onKeyDown={(event) => {
        if (event.key !== "Escape") return;
        event.preventDefault();
        event.stopPropagation();
        const reply = (event.target as HTMLElement).closest?.("[data-reply]");
        if (reply && replyFor) closeReply(replyFor);
        else props.onHide();
      }}
      style={
        geometry.narrow
          ? { left: 8, right: 8, bottom: geometry.bottom, height: geometry.height }
          : { right: geometry.right, bottom: geometry.bottom, width: geometry.width, height: geometry.height }
      }
      className="fixed z-40 flex flex-col rounded-[10px] border border-accent bg-surface text-[13px] text-text shadow-lg"
    >
      <header className="flex min-w-0 flex-none items-center gap-1.5 rounded-t-[10px] border-b border-border bg-surface-2 py-1 pr-2 pl-3">
        <h2 id={titleId} className="m-0 text-[13.5px] font-bold whitespace-nowrap">
          Ask
        </h2>
        <Tooltip
          tip="Only you see this conversation. Answers are LLM analysis, not checked; the agent was told not to run code."
          className="min-w-0 flex-1"
        >
          <span id={aboutId} tabIndex={0} className="block truncate text-[11.5px] text-muted">
            {props.about} · answers are LLM analysis
          </span>
        </Tooltip>
        <Tooltip tip="Use the full height beside the code; click again for your previous size">
          <button
            type="button"
            aria-pressed={size.full}
            onClick={() => onSize({ ...size, full: !size.full })}
            className={`${HEAD_BUTTON} aria-pressed:border-accent aria-pressed:bg-accent-bg`}
          >
            Full height
          </button>
        </Tooltip>
        <Tooltip tip="Hide the chat (Esc). Your draft and answers are kept; the Ask button or a brings it back.">
          <button type="button" aria-label="Hide the Ask chat" onClick={props.onHide} className={HEAD_BUTTON}>
            Hide
          </button>
        </Tooltip>
      </header>
      <div
        ref={list}
        role="region"
        aria-label="Conversation, oldest first"
        tabIndex={0}
        onScroll={(event) => {
          const e = event.currentTarget;
          following_.current = e.scrollTop + e.clientHeight >= e.scrollHeight - FOLLOW_PX;
        }}
        className="min-h-0 flex-1 overflow-auto overscroll-contain px-3 py-2.5"
      >
        {threads.length === 0 ? (
          <div className="grid gap-1.5 rounded-lg border border-dashed border-border p-3 text-muted">
            <p className="m-0">
              <strong>No questions yet.</strong>
            </p>
            <p className="m-0">
              Ask about the step you are on, lines you selected in Files, a flow or the whole PR. Each
              question keeps where it was asked; click that to go back there.
            </p>
            <p className="m-0">
              Under an answer, <em>Ask a follow-up</em> continues the thread: the LLM keeps its context.
            </p>
          </div>
        ) : (
          <ol aria-label="Your questions, oldest first" className="m-0 grid list-none gap-4 p-0">
            {threads.map(({ root, followUps }) => {
              const fresh = [root, ...followUps].some((q) => props.unread.has(q.id));
              return (
                <li
                  key={root.id}
                  data-thread={root.id}
                  tabIndex={-1}
                  ref={(element) => {
                    if (element) threadItems.current.set(root.id, element);
                    else threadItems.current.delete(root.id);
                  }}
                  aria-label={`Question: ${root.text}`}
                  className="grid scroll-m-2 gap-1.5 rounded-lg p-0.5 focus-visible:outline-3 focus-visible:outline-focus"
                >
                  {fresh && (
                    <span className="justify-self-end rounded-sm bg-accent px-1.5 text-[10px] font-bold text-on-accent">
                      new
                    </span>
                  )}
                  <Question question={root} root tour={tour} onGoto={props.onGoto} />
                  <ChatAnswer question={root} anchor={root.anchor} {...props} />
                  {followUps.length > 0 && (
                    <ol
                      aria-label="Follow-ups"
                      className="mt-0.5 ml-3.5 grid list-none gap-2.5 border-l-2 border-border p-0 pl-2.5"
                    >
                      {followUps.map((f) => (
                        <li key={f.id} className="grid gap-1.5">
                          <Question question={f} root={false} tour={tour} onGoto={props.onGoto} />
                          <ChatAnswer question={f} anchor={root.anchor} {...props} />
                        </li>
                      ))}
                    </ol>
                  )}
                  {replyFor === root.id ? (
                    <Reply
                      root={root}
                      draft={replyDrafts[root.id] ?? ""}
                      onDraft={(text) => setReplyDrafts((all) => ({ ...all, [root.id]: text }))}
                      onReply={props.onReply}
                      onClose={() => closeReply(root.id)}
                    />
                  ) : (
                    <div className="ml-3.5 pl-2.5">
                      <Tooltip tip="Asks in this thread: the LLM gets this question with its answers and follow-ups, and the same subject.">
                        <button
                          type="button"
                          ref={(element) => {
                            if (element) replyButtons.current.set(root.id, element);
                            else replyButtons.current.delete(root.id);
                          }}
                          onClick={() => setReplyFor(root.id)}
                          className={BUTTON}
                        >
                          Ask a follow-up
                        </button>
                      </Tooltip>
                    </div>
                  )}
                </li>
              );
            })}
          </ol>
        )}
      </div>
      <form
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          void send();
        }}
        className="flex-none rounded-b-[10px] border-t border-border bg-surface-2 px-2.5 py-2"
      >
        <div className="mb-1 flex min-w-0 flex-wrap items-center gap-1.5 text-[12px]">
          <span className="text-muted">About:</span>
          {following && subject.kind !== "pr" ? (
            <span className="inline-flex max-w-full min-w-0 items-center gap-0.5 rounded-[10px] border border-accent bg-accent-bg pl-2">
              <Tooltip tip="Your question will be about this place. It follows where you are in the review; × asks about the whole PR instead.">
                <span tabIndex={0} className="[overflow-wrap:anywhere]">
                  <SubjectText anchor={subject} tour={tour} />
                </span>
              </Tooltip>
              <Tooltip tip="Ask about the whole PR instead">
                <button
                  type="button"
                  aria-label="Remove the subject: ask about the whole PR"
                  onClick={() => {
                    props.onUnanchor();
                    field.current?.focus();
                  }}
                  className="min-h-6 min-w-6 flex-none cursor-pointer rounded-[10px] border-0 bg-transparent text-[15px] leading-none text-text hover:bg-surface"
                >
                  ×
                </button>
              </Tooltip>
            </span>
          ) : (
            <>
              <Tooltip tip="Your question will be about the whole pull request.">
                <span tabIndex={0} className="rounded-[10px] border border-accent bg-accent-bg px-2">
                  Whole PR
                </span>
              </Tooltip>
              {placeWhat && !following && (
                <Tooltip tip="Ask about where you are in the review instead">
                  <button
                    type="button"
                    onClick={() => {
                      props.onUsePlace();
                      field.current?.focus();
                    }}
                    className="cursor-pointer border-0 bg-transparent p-0 text-accent underline"
                  >
                    about {placeWhat} instead
                  </button>
                </Tooltip>
              )}
            </>
          )}
        </div>
        <div className="flex items-end gap-1.5">
          <label htmlFor={fieldId} className="sr-only">
            Your question
          </label>
          <textarea
            id={fieldId}
            ref={(element) => {
              field.current = element;
              assignRef(props.inputRef, element);
            }}
            rows={1}
            value={draft}
            placeholder={`Ask about ${s.what}…`}
            aria-describedby={error ? `${fieldId}-e` : undefined}
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
            className="max-h-40 min-h-[34px] flex-1 resize-none rounded-md border border-border bg-surface px-2 py-1.5 text-[13px]"
          />
          <Tooltip
            tip={busyTip(
              "Sends the question to the LLM in your private session. Nothing goes to GitHub.",
              action.busy,
            )}
          >
            <button type="submit" aria-disabled={action.busy || undefined} className={PRIMARY}>
              Ask
            </button>
          </Tooltip>
        </div>
        {error && (
          <p id={`${fieldId}-e`} role="alert" className="m-0 mt-1 text-[12px] text-high">
            {error}
          </p>
        )}
        <p className="m-0 mt-1 text-[11px] text-muted">
          <Kbd>Enter</Kbd> sends · <Kbd>Shift</Kbd>+<Kbd>Enter</Kbd> new line · <Kbd>Esc</Kbd> hides the chat
        </p>
      </form>
      {!geometry.narrow && (
        <>
          <SizeHandle dimension="height" geometry={geometry} size={size} onSize={onSize} />
          <SizeHandle dimension="width" geometry={geometry} size={size} onSize={onSize} />
        </>
      )}
    </section>
  );
}

/** The state or answer of a question; follow-ups take the thread's subject for "Use as review comment". */
function ChatAnswer({
  question,
  anchor,
  onAskAgain,
  onEvidence,
  onUseAsComment,
}: { question: ChatQuestion; anchor: ChatAnchor } & Pick<
  AskChatProps,
  "onAskAgain" | "onEvidence" | "onUseAsComment"
>) {
  // `Answer` passes the anchor back only to `onAskAgain`, which asks this question again in its thread.
  const shown = question as unknown as AskQuestion;
  return (
    <div className="max-w-[96%] min-w-0 text-[13px]">
      <Answer
        question={shown}
        onAskAgain={() => onAskAgain(question)}
        onEvidence={onEvidence}
        onUseAsComment={onUseAsComment && hasLines(anchor) ? () => onUseAsComment(question) : undefined}
      />
    </div>
  );
}
