// The floating Ask chat of a review (proposal AC1–AC14): it lives in the layout shared by Flows and Files, so it
// survives their navigation. Screens say where the reader is and open it; the chat asks, follows and navigates.
// The Comments pages draw the same chat over the reader's questions about the PR's comment items (RC7), with or
// without an analysis.
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useNavigate } from "react-router";
import { analysisActivity, questionsActivity, usePrTabSubject, useTabActivity } from "../../app/tabStatus";
import { useTourNotifications } from "../../app/workNotifications";
import { routes } from "../../api/generated/routes";
import type {
  AnalysisId,
  AnchoredAskRequest,
  AskAnchor,
  AskQuestion,
  Evidence,
  QuestionId,
  LineRange,
  LineSide,
  PrRef,
  Tour,
} from "../../api/generated/types";
import {
  useAskInChat,
  useMe,
  usePendingReview,
  useQuestionEvents,
  useQuestions,
  useTour,
} from "../../api/queries";
import { useAskPrQuestion, useCommentItems, usePrQuestions } from "../../api/review-loop";
import { AskChat, type ChatSize } from "../../components/ask/chat/AskChat";
import { AskLauncher } from "../../components/ask/chat/AskLauncher";
import {
  commentSubject,
  linesOf,
  subjectOf,
  threadsOf,
  unreadOf,
  WHOLE_PR,
  type ItemNumbers,
  type Subject,
} from "../../components/ask/chat/model";
import { answerComment } from "../../components/comment/prefill";
import { filesHash } from "../../components/files/model";
import { commentsPath, itemPath } from "../pr/paths";
import { blockFragment } from "../tour/sequence";
import { chatGeometry } from "./chatGeometry";

/** What a screen does for the chat: evidence links and "Use as review comment" open in its own columns. */
export interface ChatActions {
  onEvidence?: (evidence: Evidence) => void;
  /** Opens the comment composer on the anchor's lines with `text` as a draft; absent while commenting is unavailable. */
  onUseAsComment?: (anchor: AskAnchor, text: string) => void;
}

/** "Use as review comment" handed from Flows to Files: the answer's text for the composer on these lines. */
export interface AnswerDraft {
  path: string;
  side: LineSide;
  range: LineRange;
  text: string;
}

/** The chat as screens see it. */
export interface AskChatControl {
  /** Where the reader is (the composer's subject while it follows); null outside a place. */
  place: AskAnchor | null;
  setPlace: (place: AskAnchor | null) => void;
  /** Opens the chat about the place (or `anchor`), or at a thread; focuses its field. Nothing is sent. */
  open: (options?: { anchor?: AskAnchor; thread?: QuestionId }) => void;
  setActions: (actions: ChatActions) => void;
  /** Says something politely to screen readers (e.g. where "Use as review comment" moved the review). */
  announce: (text: string) => void;
}

const NONE: AskChatControl = {
  place: null,
  setPlace: () => {},
  open: () => {},
  setActions: () => {},
  announce: () => {},
};
const Context = createContext<AskChatControl>(NONE);

/** The review's chat; a no-op outside a review layout (component tests of a screen). */
export const useAskChat = () => useContext(Context);

/** Says where the reader is while the screen shows it. */
export function useAskPlace(place: AskAnchor | null): void {
  const { setPlace } = useAskChat();
  const key = place === null ? null : JSON.stringify(place);
  useEffect(() => {
    setPlace(key === null ? null : (JSON.parse(key) as AskAnchor));
  }, [key, setPlace]);
}

const read = <T,>(storage: () => Storage, key: string, fallback: T): T => {
  try {
    const raw = storage().getItem(key);
    return raw === null ? fallback : ({ ...fallback, ...(JSON.parse(raw) as object) } as T);
  } catch {
    return fallback;
  }
};
const write = (storage: () => Storage, key: string, value: unknown) => {
  try {
    if (value === null) storage().removeItem(key);
    else storage().setItem(key, JSON.stringify(value));
  } catch {
    // Blocked site data: the chat works without remembering.
  }
};
const local = () => window.localStorage;
const session = () => window.sessionStorage;

interface Remembered extends ChatSize {
  open: boolean;
}
const FIRST: Remembered = { open: false, width: null, height: null, full: false };

/** Where a comment item opens: its question in Comments, or Comments when its number is not known. */
const commentLink = (pr: PrRef, item: string, numbers: ItemNumbers) => {
  const no = numbers.get(item);
  return no === undefined ? commentsPath(pr) : itemPath(pr, no);
};

// Where a subject chip goes: lines (a step's too) are selected in Files, comment items in Comments, the rest in Flows.
const linkOf = (id: AnalysisId, anchor: AskAnchor, tour: Tour, numbers: ItemNumbers): string => {
  const base = `/analyses/${encodeURIComponent(id)}`;
  const lines = anchor.kind === "step" && anchor.lines ? linesOf(anchor, tour) : null;
  if (lines)
    return `${base}/files${filesHash({ file: lines.path, side: lines.side, line: lines.from, end: lines.to ?? lines.from })}`;
  switch (anchor.kind) {
    case "comment_item":
      return commentLink(tour.analysis.pr, anchor.item, numbers);
    case "pr":
      return `${base}#${blockFragment({ block: "pr_context" })}`;
    case "flow":
      return `${base}#${blockFragment({ block: "flow_intro", flow_id: anchor.flow_id })}`;
    case "step":
      return `${base}#${blockFragment({ block: "step", step_id: anchor.step_id })}`;
    case "lines":
      return `${base}/files${filesHash({
        file: anchor.path,
        side: anchor.side,
        ...(anchor.range ? { line: anchor.range.start_line, end: anchor.range.end_line } : {}),
      })}`;
  }
};

/** Browser notifications and the tab title's work status, for Flows and Files alike. */
function ReviewActivity({ tour, questions }: { tour: Tour; questions: readonly AskQuestion[] | undefined }) {
  useTabActivity("analysis", analysisActivity(tour.analysis.state));
  useTabActivity("ask", questionsActivity(questions ?? []));
  usePrTabSubject(tour.analysis.pr);
  useTourNotifications(tour, questions);
  return null;
}

/** Where a chat's questions come from and how its subjects are named and opened. */
interface ChatSource {
  /** Drafts, replies and seen answers are remembered per source (an analysis, or a PR's comment items). */
  key: string;
  questions: { data: AskQuestion[] | undefined; isSuccess: boolean };
  ask: (body: AnchoredAskRequest) => Promise<AskQuestion>;
  /** Names a subject; null while the source is not ready (its tour loads): the chat is not drawn yet. */
  describe: ((anchor: AskAnchor) => Subject) | null;
  /** In-app link of a subject. */
  linkOf: (anchor: AskAnchor) => string;
  /** "Use as review comment" is offered (the user's pending review accepts comments). */
  commenting: boolean;
  about: string;
  notice: string | null;
  /** Tab status and notifications of the source's work. */
  activity: ReactNode;
}

/** Provides the chat to the review's screens and draws it (or its launcher) over them. */
export function AskChatProvider({ analysisId, children }: { analysisId: AnalysisId; children: ReactNode }) {
  const tour = useTour(analysisId);
  const questions = useQuestions(analysisId);
  const stream = useQuestionEvents(analysisId);
  const ask = useAskInChat(analysisId);
  const pending = usePendingReview(analysisId);
  const theTour = tour.data;
  // Comment items are named "Comment C14" with the PR's items, read only when a question is about one.
  const aboutItems = (questions.data ?? []).some((q) => q.anchor.kind === "comment_item");
  const items = useCommentItems(theTour?.analysis.pr ?? null, aboutItems);
  const numbers = useMemo<ItemNumbers>(
    () => new Map((items.data ?? []).map((item) => [item.id, item.no])),
    [items.data],
  );
  const source: ChatSource = {
    key: analysisId,
    questions,
    ask: (body) => ask.mutateAsync(body),
    describe: theTour ? (anchor) => subjectOf(anchor, theTour, numbers) : null,
    linkOf: (anchor) => (theTour ? linkOf(analysisId, anchor, theTour, numbers) : ""),
    commenting: pending.data?.commenting_available === true,
    about: theTour ? `your private chat about PR #${theTour.analysis.pr.number}` : "",
    notice: stream.lost
      ? "The connection that delivers answers was lost. Reconnecting; answers already given are read again."
      : null,
    activity: theTour ? <ReviewActivity tour={theTour} questions={questions.data} /> : null,
  };
  return <ChatShell source={source}>{children}</ChatShell>;
}

/** The tab title's Ask status of a PR's comment-item questions. */
function PrQuestionActivity({ questions }: { questions: readonly AskQuestion[] | undefined }) {
  useTabActivity("ask", questionsActivity(questions ?? []));
  return null;
}

/**
 * The chat of the Comments pages: the reader's questions about the PR's comment items, asked on the PR (RC7) and
 * read again while one is open; with an analysis of the head its Ask session answers, else a question pass.
 */
export function PrAskChatProvider({
  pr,
  numbers,
  children,
}: {
  pr: PrRef;
  /** `C<no>` of the PR's items. */
  numbers: ItemNumbers;
  children: ReactNode;
}) {
  const questions = usePrQuestions(pr);
  const ask = useAskPrQuestion(pr);
  const source: ChatSource = {
    key: `pr:${pr.repo.owner}/${pr.repo.name}/${pr.number}`,
    questions,
    ask: (body) => ask.mutateAsync(body),
    describe: (anchor) => (anchor.kind === "comment_item" ? commentSubject(anchor.item, numbers) : WHOLE_PR),
    linkOf: (anchor) =>
      anchor.kind === "comment_item" ? commentLink(pr, anchor.item, numbers) : commentsPath(pr),
    commenting: false,
    about: `your private chat about the comments of PR #${pr.number}`,
    notice: null,
    activity: <PrQuestionActivity questions={questions.data} />,
  };
  return <ChatShell source={source}>{children}</ChatShell>;
}

/** The chat's state, its context for screens and its drawing, over one source. */
function ChatShell({ source, children }: { source: ChatSource; children: ReactNode }) {
  const [place, setPlaceState] = useState<AskAnchor | null>(null);
  const actions = useRef<ChatActions>({});
  const setPlace = useCallback((next: AskAnchor | null) => setPlaceState(next), []);
  const setActions = useCallback((next: ChatActions) => {
    actions.current = next;
  }, []);
  const navigate = useNavigate();
  const me = useMe();
  const login = me.data?.user?.login ?? "anonymous";
  const { questions } = source;
  const all = questions.data ?? [];

  const layoutKey = `diffpilot.askChat.${login}`;
  const draftKey = `diffpilot.askChat.draft.${login}.${source.key}`;
  const seenKey = `diffpilot.askChat.seen.${login}.${source.key}`;
  const repliesKey = `diffpilot.askChat.replies.${login}.${source.key}`;
  const [remembered, setRemembered] = useState(() => read(local, layoutKey, FIRST));
  const [draft, setDraft] = useState(() => read(session, draftKey, { text: "" }).text);
  const [replies, setReplies] = useState(() => read<Record<QuestionId, string>>(session, repliesKey, {}));
  // The subject: the place, unless × chose the whole PR; an Ask button's own subject until the place changes.
  const [wholePr, setWholePr] = useState(false);
  const placeKey = JSON.stringify(place);
  const [chosen, setChosen] = useState<{ anchor: AskAnchor; at: string } | null>(null);
  const [focusThread, setFocusThread] = useState<{ id: QuestionId; nonce: number } | null>(null);
  const field = useRef<HTMLTextAreaElement>(null);
  const opener = useRef<HTMLElement | null>(null);
  const launcher = useRef<HTMLDivElement>(null);

  // Outcomes the reader has seen; on a first visit everything already answered counts as seen.
  const [seen, setSeen] = useState<ReadonlySet<QuestionId> | null>(() => {
    const stored = read<{ ids: QuestionId[] | null }>(local, seenKey, { ids: null }).ids;
    return stored ? new Set(stored) : null;
  });
  // Everything remembered is per user: read again once the signed-in user is known.
  const [readFor, setReadFor] = useState(login);
  if (readFor !== login) {
    setReadFor(login);
    setRemembered(read(local, layoutKey, FIRST));
    setDraft(read(session, draftKey, { text: "" }).text);
    setReplies(read<Record<QuestionId, string>>(session, repliesKey, {}));
    const stored = read<{ ids: QuestionId[] | null }>(local, seenKey, { ids: null }).ids;
    setSeen(stored ? new Set(stored) : null);
  } else if (seen === null && questions.isSuccess && me.isSuccess) setSeen(new Set(unreadOf(all, new Set())));
  const unread = remembered.open ? [] : unreadOf(all, seen ?? new Set());
  // Threads with news, marked when the chat opens.
  const [marked, setMarked] = useState<ReadonlySet<QuestionId>>(new Set());

  // Arrivals are announced politely; the list itself is not live.
  const [states, setStates] = useState<Record<QuestionId, string>>({});
  const [announcement, setAnnouncement] = useState("");
  const now = Object.fromEntries(all.map((q) => [q.id, q.state]));
  if (questions.isSuccess && JSON.stringify(now) !== JSON.stringify(states)) {
    const arrived = all.find(
      (q) =>
        states[q.id] !== undefined &&
        states[q.id] !== q.state &&
        (q.state === "answered" || q.state === "failed"),
    );
    setStates(now);
    if (arrived)
      setAnnouncement(
        `${arrived.state === "answered" ? "Answer arrived for" : "No answer for"}: ${arrived.text.slice(0, 80)}`,
      );
  }

  const remember = (next: Remembered) => {
    setRemembered(next);
    write(local, layoutKey, next);
  };
  const markSeen = () => {
    const ids = new Set(unreadOf(all, new Set()));
    setSeen(ids);
    write(local, seenKey, { ids: [...ids] });
  };
  const show = (thread?: QuestionId) => {
    const active = document.activeElement as HTMLElement | null;
    if (!remembered.open || !active?.closest("[data-ask-chat]"))
      opener.current =
        active && active !== document.body && !active.closest("[data-ask-chat]") ? active : null;
    if (!remembered.open) setMarked(new Set(unread));
    markSeen();
    remember({ ...remembered, open: true });
    if (thread) setFocusThread((f) => ({ id: thread, nonce: (f?.nonce ?? 0) + 1 }));
    else requestAnimationFrame(() => field.current?.focus());
  };
  const hide = () => {
    markSeen();
    setMarked(new Set());
    remember({ ...remembered, open: false });
    const back = opener.current;
    requestAnimationFrame(() =>
      (back && document.contains(back) ? back : launcher.current?.querySelector("button"))?.focus(),
    );
  };

  // `a`, an Ask button or Open in chat: the place (or the button's own subject) becomes the subject again.
  const open = (options: { anchor?: AskAnchor; thread?: QuestionId } = {}) => {
    setWholePr(false);
    setChosen(options.anchor ? { anchor: options.anchor, at: placeKey } : null);
    show(options.thread);
  };
  // Said aloud and shown in the chat for a few seconds.
  const [status, setStatus] = useState<{ text: string } | null>(null);
  const announce = useCallback((text: string) => {
    setAnnouncement(text);
    setStatus({ text });
  }, []);
  useEffect(() => {
    if (!status) return;
    const timer = setTimeout(() => setStatus(null), 6000);
    return () => clearTimeout(timer);
  }, [status]);
  // Screens see a context that changes only with the place: `open` reads the latest state through a ref.
  const latestOpen = useRef(open);
  useLayoutEffect(() => {
    latestOpen.current = open;
  });
  const stableOpen = useCallback((options?: Parameters<typeof open>[0]) => latestOpen.current(options), []);
  const control = useMemo<AskChatControl>(
    () => ({ place, setPlace, open: stableOpen, setActions, announce }),
    [place, setPlace, stableOpen, setActions, announce],
  );

  const subject: AskAnchor = wholePr
    ? { kind: "pr" }
    : chosen && chosen.at === placeKey
      ? chosen.anchor
      : (place ?? { kind: "pr" });

  // Where the chat is drawn: measured from the columns, again on resize and when the dock grows.
  const [box, setBox] = useState(() =>
    chatGeometry({ width: 1600, height: 900 }, null, null, null, remembered),
  );
  useLayoutEffect(() => {
    const measure = () => {
      const rect = (selector: string) => document.querySelector(selector)?.getBoundingClientRect() ?? null;
      const view = {
        width: document.documentElement.clientWidth || window.innerWidth,
        height: document.documentElement.clientHeight || window.innerHeight,
      };
      const next = chatGeometry(
        view,
        rect("[data-workspace]"),
        rect('[data-column="inspector"]'),
        rect("[data-inspector] [data-dock]")?.top ?? null,
        remembered,
      );
      setBox((old) => (JSON.stringify(old) === JSON.stringify(next) ? old : next));
      const inspectorHidden = (rect('[data-column="inspector"]')?.width ?? 0) < (2 * next.minWidth) / 3;
      // The launcher's corner stays free: the Inspector (or, without it, the centre) ends above it.
      const launcherBox = launcher.current?.querySelector("button")?.getBoundingClientRect();
      const launcherPad = launcherBox?.height ? `${Math.ceil(view.height - launcherBox.top) + 8}px` : "0px";
      document.documentElement.style.setProperty(
        "--ask-chat-pad",
        remembered.open
          ? inspectorHidden
            ? `${next.height + 24}px`
            : "0px"
          : inspectorHidden
            ? launcherPad
            : "0px",
      );
      document.documentElement.style.setProperty("--ask-launcher-pad", remembered.open ? "0px" : launcherPad);
    };
    measure();
    window.addEventListener("resize", measure);
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    const watch = () => {
      observer?.disconnect();
      for (const selector of [
        "[data-workspace]",
        '[data-column="inspector"]',
        "[data-inspector] [data-dock]",
      ]) {
        const element = document.querySelector(selector);
        if (element) observer?.observe(element);
      }
    };
    watch();
    // Columns and the dock come and go with navigation.
    const mutations =
      typeof MutationObserver === "undefined"
        ? null
        : new MutationObserver(() => {
            watch();
            measure();
          });
    mutations?.observe(document.body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["data-column"],
    });
    return () => {
      window.removeEventListener("resize", measure);
      observer?.disconnect();
      mutations?.disconnect();
      document.documentElement.style.removeProperty("--ask-chat-pad");
      document.documentElement.style.removeProperty("--ask-launcher-pad");
    };
  }, [remembered]);

  const { describe, commenting } = source;
  if (!describe) return <Context.Provider value={control}>{children}</Context.Provider>;
  const send = (body: AnchoredAskRequest) => source.ask(body);
  const busy = all.some((q) => q.state === "answering");
  return (
    <Context.Provider value={control}>
      {children}
      {source.activity}
      <p className="sr-only" aria-live="polite">
        {announcement}
      </p>
      {remembered.open ? (
        <AskChat
          describe={describe}
          threads={threadsOf(all.filter((q) => q.mine !== false))}
          subject={subject}
          following={!wholePr}
          placeWhat={place ? describe(place).what : null}
          draft={draft}
          onDraft={(text) => {
            setDraft(text);
            write(session, draftKey, text ? { text } : null);
          }}
          replyDrafts={replies}
          onReplyDraft={(root, text) => {
            const next = { ...replies, [root]: text };
            if (!text) delete next[root];
            setReplies(next);
            write(session, repliesKey, Object.keys(next).length ? next : null);
          }}
          signInHref={routes.authLogin.path}
          status={status?.text ?? null}
          onSend={(text) => send({ text, anchor: subject })}
          onReply={(root, text) => send({ text, anchor: root.anchor, parent_id: root.id })}
          // The same question again in the same place: a first question as a first question, a follow-up in its
          // thread; the retry then replaces the failed entry (`withoutRetried` in the questions query).
          onAskAgain={(q) =>
            send({ text: q.text, anchor: q.anchor, ...(q.parent_id ? { parent_id: q.parent_id } : {}) })
          }
          onUnanchor={() => setWholePr(true)}
          onUsePlace={() => {
            setWholePr(false);
            setChosen(null);
          }}
          onGoto={(anchor) => void navigate(source.linkOf(anchor))}
          onEvidence={(evidence) =>
            actions.current.onEvidence
              ? actions.current.onEvidence(evidence)
              : void navigate(
                  source.linkOf({
                    kind: "lines",
                    path: evidence.file,
                    side: evidence.side,
                    range: { start_line: evidence.line, end_line: evidence.line },
                  }),
                )
          }
          onUseAsComment={
            commenting
              ? (q) => {
                  const root = threadsOf(all).find(
                    (t) => t.root.id === q.id || t.followUps.some((f) => f.id === q.id),
                  )?.root;
                  actions.current.onUseAsComment?.(
                    root?.anchor ?? q.anchor,
                    answerComment(q.answer?.answer ?? ""),
                  );
                }
              : undefined
          }
          onHide={hide}
          size={remembered}
          geometry={box}
          onSize={(size) => remember({ ...remembered, ...size })}
          about={source.about}
          unread={marked}
          focusThread={focusThread}
          inputRef={field}
          notice={source.notice}
        />
      ) : (
        <div ref={launcher}>
          <AskLauncher unread={unread.length} busy={busy} onOpen={() => show()} />
        </div>
      )}
    </Context.Provider>
  );
}
