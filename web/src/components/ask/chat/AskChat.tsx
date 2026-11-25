import { useEffect, useId, useLayoutEffect, useRef, useState, type Ref, type SetStateAction } from "react";
import type { AskAnchor, AskQuestion, Evidence, QuestionId } from "../../../api/generated/types";
import { busyTip } from "../../button/button";
import { Kbd } from "../../help/Kbd";
import { assignRef, growField, useAction } from "../../inspector/action";
import { Tooltip } from "../../tooltip/Tooltip";
import { remarkLines, remarkQuote, type ChatThread, type Subject } from "./model";
import { AskThread, NotSent, notSent, sends } from "./AskThread";
import { AskWindowHeader, CornerHandle, SizeHandle } from "./AskWindow";

export type ChatCorner = "bottom-right" | "bottom-left" | "top-right" | "top-left";
export type ChatPosition = ChatCorner | { right: number; bottom: number };

/** The chat's remembered size: null = the default; `full` = the whole height above the dock. */
export interface ChatSize {
  width: number | null;
  height: number | null;
  full: boolean;
  phoneFull?: boolean;
  position?: ChatPosition;
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
  /** Use the free edge when the opposite edge is against a viewport limit. */
  resizeFromRight?: boolean;
  resizeFromBottom?: boolean;
}

/** Props of the floating Ask chat. */
export interface AskChatProps {
  /** Names a subject (chip, placeholder, tips): `subjectOf` over the tour, or the comment items of a PR. */
  describe: (anchor: AskAnchor) => Subject;
  /** The user's threads of the analysis, oldest first (`threadsOf`). */
  threads: ChatThread[];
  /** What the next question is about. */
  subject: AskAnchor;
  /** The subject follows the reader's place (× asks about the whole PR instead). */
  following: boolean;
  /** "this step", "these lines"…: where the reader is, offered while the subject is the whole PR by choice. */
  placeWhat: string | null;
  draft: string;
  onDraft: (text: SetStateAction<string>) => void;
  /** Follow-up drafts by thread (the first question's id); kept by the caller across Hide and reload. */
  replyDrafts: Readonly<Record<QuestionId, string>>;
  onReplyDraft: (root: QuestionId, text: SetStateAction<string>) => void;
  /** A fixed clock for observed state transitions (tests). */
  now?: number;
  /** A short note about what just happened (e.g. where "Use as review comment" moved the review); said aloud by the caller. */
  status?: string | null;
  /** The sign-in page, offered when a question is refused for a lost sign-in. */
  signInHref: string;
  /** Sends a new question about `subject`; a rejection keeps the text and shows its message. */
  onSend: (text: string) => Promise<unknown> | void;
  /** Sends a follow-up in the thread of `root` (same subject; the LLM gets the thread). */
  onReply: (root: AskQuestion, text: string) => Promise<unknown> | void;
  /** Asks a question without an answer again, in its thread. */
  onAskAgain: (question: AskQuestion) => Promise<unknown> | void;
  onUnanchor: () => void;
  /** Why the whole PR cannot be the subject here: × is off with this hint, and a question about it is not sent. */
  wholePrUnavailable?: string;
  onUsePlace: () => void;
  /** Opens what a thread is about (its subject chip); the chat stays open. */
  onGoto: (anchor: AskAnchor) => void;
  onEvidence?: (evidence: Evidence) => void;
  /** "Use as review comment" on an answer about lines; absent while commenting is unavailable. */
  onUseAsComment?: (question: AskQuestion) => void;
  /** Hides the chat (Esc); its drafts and answers are kept. */
  onHide: () => void;
  size: ChatSize;
  geometry: ChatGeometry;
  onSize: (size: ChatSize) => void;
  /** The privacy line under the title. */
  about: string;
  /** Questions whose outcome the reader has not seen: their threads are marked new. */
  unread: ReadonlySet<QuestionId>;
  /** Keeps the marker on an answer seen during this opening. */
  marked?: ReadonlySet<QuestionId>;
  /** A thread to scroll to and focus ("Open in chat"); `nonce` repeats it. */
  focusThread: { id: QuestionId; nonce: number } | null;
  /** Records an unread outcome that entered the conversation viewport. */
  onSeen?: (ids: QuestionId[]) => void;
  inputRef?: Ref<HTMLTextAreaElement>;
  /** A problem with the answers (e.g. the lost answer stream), said above the conversation. */
  notice?: string | null;
}

const PRIMARY =
  "min-h-8 cursor-pointer rounded-md border border-accent bg-accent px-3 text-[13px] font-semibold text-on-accent aria-disabled:cursor-wait aria-disabled:opacity-60";
// Within this many px of the end the reader counts as following the newest entry.
const FOLLOW_PX = 24;
type OutcomeTarget = { root: QuestionId; id: QuestionId };

/**
 * The floating Ask chat (proposal AC2–AC10): one private conversation per analysis and user, threads oldest first
 * with their subject chips and follow-ups, the composer whose subject follows the reader, Full height, Hide and
 * resizing. Non-modal: focus is not trapped.
 */
export function AskChat(props: AskChatProps) {
  const { describe, threads, subject, following, placeWhat, draft, onDraft, geometry, size, onSize } = props;
  const titleId = useId();
  const aboutId = useId();
  const fieldId = useId();
  const list = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLTextAreaElement>(null);
  const following_ = useRef(true);
  const lastThread = useRef<QuestionId | undefined>(undefined);
  const action = useAction();
  const [empty, setEmpty] = useState(false);
  const [wholePrRefused, setWholePrRefused] = useState(false);
  const [replyFor, setReplyFor] = useState<QuestionId | null>(null);
  const [disclosed, setDisclosed] = useState<Partial<Record<QuestionId, boolean>>>({});
  const [focusRequest, setFocusRequest] = useState<AskChatProps["focusThread"]>(null);
  if (props.focusThread !== focusRequest) {
    setFocusRequest(props.focusThread);
    if (props.focusThread) setDisclosed({ ...disclosed, [props.focusThread.id]: true });
  }
  const threadItems = useRef(new Map<QuestionId, HTMLLIElement>());
  const s = describe(subject);
  const remark = remarkLines(draft, subject);
  const chip = remark ? `Remark · ${s.text}` : s.text;
  const cards = threads.map(({ root, followUps }, index) => ({
    root,
    followUps,
    number: `Q${index + 1}`,
    questions: [root, ...followUps],
  }));
  const all = cards.flatMap((card) => card.questions.map((question) => ({ card, question })));
  const newestThread = cards.at(-1)?.root.id;
  const ordered = [...all].sort(
    (a, b) => Date.parse(a.question.created_at) - Date.parse(b.question.created_at),
  );
  const firstActive =
    ordered.find(({ question }) => question.state === "answering") ??
    ordered.find(({ question }) => question.state === "pending");

  // Track state changes seen in this window. The server only sends the time the question was asked.
  const states = Object.fromEntries(all.map(({ question }) => [question.id, question.state]));
  const [known, setKnown] = useState(states);
  const [jump, setJump] = useState<OutcomeTarget | null>(null);
  const jumpRequest = useRef<OutcomeTarget | null>(null);
  const pastStates = useRef<Record<QuestionId, string>>({});
  const [starts, setStarts] = useState<Record<QuestionId, number>>({});
  useEffect(() => {
    const frame = requestAnimationFrame(() => {
      const next = Object.fromEntries(
        threads.flatMap(({ root, followUps }) =>
          [root, ...followUps].map((question) => [question.id, question.state]),
        ),
      );
      const began = Object.keys(next).filter(
        (id) => pastStates.current[id] === "pending" && next[id] === "answering",
      );
      pastStates.current = next;
      if (!began.length) return;
      const now = props.now ?? Date.now();
      setStarts((previous) => ({ ...previous, ...Object.fromEntries(began.map((id) => [id, now])) }));
    });
    return () => cancelAnimationFrame(frame);
  }, [threads, props.now]);
  if (JSON.stringify(states) !== JSON.stringify(known)) {
    const outcomes = all.filter(
      ({ question: q }) =>
        known[q.id] !== undefined &&
        known[q.id] !== q.state &&
        (q.state === "answered" || q.state === "failed"),
    );
    if (outcomes.length) {
      setDisclosed(
        Object.fromEntries(
          Object.entries(disclosed).filter(([id]) => !outcomes.some(({ card }) => card.root.id === id)),
        ),
      );
    }
    setKnown(states);
  }
  const outcomeOf = ({ root, id }: OutcomeTarget) => {
    const card = threadItems.current.get(root);
    return Array.from(card?.querySelectorAll<HTMLElement>("[data-ask-outcome]") ?? []).find(
      (element) => element.dataset.askOutcome === id,
    );
  };
  const inView = (target: OutcomeTarget) => {
    const at = outcomeOf(target)?.getBoundingClientRect();
    const box = list.current?.getBoundingClientRect();
    return Boolean(at?.height && box && at.top < box.bottom && at.bottom > box.top);
  };

  const markVisible = () => {
    const seen: QuestionId[] = [];
    let first: OutcomeTarget | null = null;
    for (const { card, question } of ordered) {
      if (!props.unread.has(question.id)) continue;
      const target = { root: card.root.id, id: question.id };
      if (inView(target)) seen.push(question.id);
      else first ??= target;
    }
    setJump((current) => (current?.id === first?.id ? current : first));
    if (seen.length) props.onSeen?.(seen);
  };

  // Follows the newest entry unless the reader scrolled up; a question just sent always shows.
  useLayoutEffect(() => {
    const element = list.current;
    const added = newestThread !== lastThread.current;
    lastThread.current = newestThread;
    if (element && (added || following_.current)) element.scrollTop = element.scrollHeight;
    const target = jumpRequest.current;
    if (target) {
      jumpRequest.current = null;
      const item = threadItems.current.get(target.root);
      (outcomeOf(target) ?? item)?.scrollIntoView?.({ block: "nearest" });
      item?.focus({ preventScroll: true });
    }
    if (!jump && !props.unread.size) return;
    const frame = requestAnimationFrame(markVisible);
    return () => cancelAnimationFrame(frame);
  });
  const focusId = props.focusThread?.id;
  const focusNonce = props.focusThread?.nonce;
  useEffect(() => {
    if (!focusId) return;
    following_.current = false;
    const item = threadItems.current.get(focusId);
    item?.scrollIntoView?.({ block: "start" });
    item?.focus({ preventScroll: true });
  }, [focusId, focusNonce]);

  const send = async () => {
    if (!draft.replace(remarkQuote, "").trim()) {
      setEmpty(true);
      field.current?.focus();
      return;
    }
    if (subject.kind === "pr" && props.wholePrUnavailable) {
      setWholePrRefused(true);
      field.current?.focus();
      return;
    }
    if (await action.run(() => props.onSend(draft.trim()))) {
      onDraft((current) => (current === draft ? "" : current));
      requestAnimationFrame(() => growField(field.current));
    }
  };
  const closeReply = (id: QuestionId) => {
    const item = threadItems.current.get(id);
    if (item?.contains(document.activeElement)) {
      requestAnimationFrame(() => item.querySelector<HTMLButtonElement>("[data-reply-button]")?.focus());
    }
    setReplyFor((current) => (current === id ? null : current));
  };
  const error = empty
    ? "Write a question first."
    : wholePrRefused && subject.kind === "pr"
      ? (props.wholePrUnavailable ?? null)
      : action.error && notSent(action.error, action.code);

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
          ? { left: 0, right: 0, bottom: 0, height: geometry.height }
          : { right: geometry.right, bottom: geometry.bottom, width: geometry.width, height: geometry.height }
      }
      className={`fixed z-40 flex flex-col border border-accent bg-surface text-[13px] text-text shadow-lg ${geometry.narrow ? "rounded-t-[10px]" : "rounded-[10px]"}`}
    >
      <AskWindowHeader
        {...{ titleId, aboutId, geometry, size, onSize }}
        about={props.about}
        onHide={props.onHide}
      />
      {props.notice && (
        <p
          role="status"
          className="m-0 flex-none border-b border-border bg-med-bg px-3 py-1 text-[12px] before:mr-1 before:content-['⚠']"
        >
          {props.notice}
        </p>
      )}
      <div
        ref={list}
        role="region"
        aria-label="Conversation, oldest first"
        tabIndex={0}
        onScroll={(event) => {
          const e = event.currentTarget;
          following_.current = e.scrollTop + e.clientHeight >= e.scrollHeight - FOLLOW_PX;
          markVisible();
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
              Only you see this conversation. Answers are LLM analysis. Follow up inside a card to keep the
              thread’s context.
            </p>
          </div>
        ) : (
          <ol aria-label="Your questions, oldest first" className="m-0 grid list-none gap-4 p-0">
            {cards.map(({ root, followUps, number, questions }, index) => {
              const fresh = questions.some((q) => props.unread.has(q.id) || props.marked?.has(q.id));
              const replyOpen = replyFor === root.id;
              const expanded =
                replyOpen ||
                (disclosed[root.id] ??
                  (index === cards.length - 1 || fresh || questions.some((q) => q.state !== "answered")));
              return (
                <AskThread
                  key={root.id}
                  root={root}
                  followUps={followUps}
                  number={number}
                  fresh={fresh}
                  expanded={expanded}
                  replyOpen={replyOpen}
                  onToggle={() => {
                    if (replyOpen) setReplyFor(null);
                    setDisclosed({ ...disclosed, [root.id]: !expanded });
                  }}
                  register={(element) => {
                    if (element) threadItems.current.set(root.id, element);
                    else threadItems.current.delete(root.id);
                  }}
                  answeringSince={(q) => starts[q.id]}
                  waitsFor={(q) => (firstActive?.question.id === q.id ? undefined : firstActive?.card.number)}
                  describe={describe}
                  onGoto={props.onGoto}
                  onAskAgain={props.onAskAgain}
                  onEvidence={props.onEvidence}
                  onUseAsComment={props.onUseAsComment}
                  signInHref={props.signInHref}
                  replyDraft={props.replyDrafts[root.id] ?? ""}
                  onReplyDraft={(text) => props.onReplyDraft(root.id, text)}
                  onReply={props.onReply}
                  onOpenReply={() => setReplyFor(root.id)}
                  onCloseReply={() => closeReply(root.id)}
                />
              );
            })}
          </ol>
        )}
      </div>
      {props.status && (
        <p
          aria-hidden="true"
          className="m-0 flex-none border-t border-border bg-accent-bg px-3 py-1 text-[12px]"
        >
          {props.status}
        </p>
      )}
      {jump && props.unread.has(jump.id) && (
        <div className="relative h-0 flex-none">
          <Tooltip
            tip="Scrolls to the answer that arrived out of view"
            className="absolute bottom-2 left-1/2 -translate-x-1/2"
          >
            <button
              type="button"
              onClick={() => {
                following_.current = false;
                jumpRequest.current = jump;
                setDisclosed({ ...disclosed, [jump.root]: true });
              }}
              className="cursor-pointer rounded-full border border-accent bg-accent px-3 py-0.5 text-[12px] font-semibold text-on-accent shadow"
            >
              ↓ New answer
            </button>
          </Tooltip>
        </div>
      )}
      <form
        aria-labelledby={`${fieldId}-label`}
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          void send();
        }}
        className="flex-none rounded-b-[10px] border-t border-border bg-surface-2 px-2.5 py-2"
      >
        <div className="mb-1 flex min-w-0 flex-wrap items-center gap-1.5 text-[12px]">
          <span id={`${fieldId}-label`} className="font-semibold">
            New question
          </span>
          <span className="text-muted">About:</span>
          {following && subject.kind !== "pr" ? (
            <span className="inline-flex max-w-full min-w-0 items-center gap-0.5 rounded-[10px] border border-accent bg-accent-bg pl-2">
              <Tooltip
                tip={
                  s.fileLabel
                    ? `${s.fileLabel} · ${s.tip}`
                    : "Your question will be about this place. It follows where you are in the review; × asks about the whole PR instead."
                }
              >
                <span
                  role="group"
                  aria-label={s.fileLabel ? `${chip} · ${s.fileLabel}` : chip}
                  tabIndex={0}
                  className="[overflow-wrap:anywhere]"
                >
                  {chip}
                </span>
              </Tooltip>
              <Tooltip tip={props.wholePrUnavailable ?? "Ask about the whole PR instead"}>
                <button
                  type="button"
                  aria-label="Remove the subject: ask about the whole PR"
                  aria-disabled={props.wholePrUnavailable ? true : undefined}
                  onClick={() => {
                    if (props.wholePrUnavailable) return;
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
              <Tooltip
                tip={props.wholePrUnavailable ?? "Your question will be about the whole pull request."}
              >
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
                    className="min-h-6 cursor-pointer border-0 bg-transparent p-0 text-accent underline"
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
              setWholePrRefused(false);
              growField(event.target);
            }}
            onKeyDown={(event) => {
              if (!sends(event)) return;
              event.preventDefault();
              void send();
            }}
            className={`min-h-[34px] flex-1 resize-none rounded-md border border-border bg-surface px-2 py-1.5 text-[13px] ${geometry.narrow ? "max-h-[20dvh]" : "max-h-40"}`}
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
          <NotSent
            id={`${fieldId}-e`}
            text={error}
            signIn={action.code === "unauthenticated" && !empty ? props.signInHref : undefined}
          />
        )}
        <p className="m-0 mt-1 text-[11px] text-muted">
          <Kbd>Enter</Kbd> sends · <Kbd>Shift</Kbd>+<Kbd>Enter</Kbd> new line · <Kbd>Esc</Kbd> hides the chat
        </p>
      </form>
      {!geometry.narrow && (
        <>
          <SizeHandle dimension="height" geometry={geometry} size={size} onSize={onSize} />
          <SizeHandle dimension="width" geometry={geometry} size={size} onSize={onSize} />
          <CornerHandle geometry={geometry} size={size} onSize={onSize} />
        </>
      )}
    </section>
  );
}
