// The floating Ask chat of a review (proposal AC1–AC14): it lives in the layout shared by Flows and Files, so it
// survives their navigation. Screens say where the reader is and open it; the chat asks, follows and navigates.
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useNavigate } from "react-router";
import type { AnalysisId, AskAnchor, AskQuestion, Evidence, QuestionId } from "../../api/generated/types";
import {
  useAskInChat,
  useMe,
  usePendingReview,
  useQuestionEvents,
  useQuestions,
  useTour,
} from "../../api/queries";
import { AskChat, type ChatSize } from "../../components/ask/chat/AskChat";
import { AskLauncher } from "../../components/ask/chat/AskLauncher";
import { subjectOf, threadsOf, unreadOf } from "../../components/ask/chat/model";
import { answerComment } from "../../components/comment/prefill";
import { filesHash } from "../../components/files/model";
import { blockFragment } from "../tour/sequence";
import { chatGeometry } from "./chatGeometry";

/** What a screen does for the chat: evidence links and "Use as review comment" open in its own columns. */
export interface ChatActions {
  onEvidence?: (evidence: Evidence) => void;
  /** Opens the comment composer on the anchor's lines with `text` as a draft; absent while commenting is unavailable. */
  onUseAsComment?: (anchor: AskAnchor, text: string) => void;
}

/** The chat as screens see it. */
export interface AskChatControl {
  /** Where the reader is (the composer's subject while it follows); null outside a place. */
  place: AskAnchor | null;
  setPlace: (place: AskAnchor | null) => void;
  /** Opens the chat about the place (or `anchor`), or at a thread; focuses its field. Nothing is sent. */
  open: (options?: { anchor?: AskAnchor; thread?: QuestionId }) => void;
  setActions: (actions: ChatActions) => void;
}

const NONE: AskChatControl = { place: null, setPlace: () => {}, open: () => {}, setActions: () => {} };
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

const linkOf = (id: AnalysisId, anchor: AskAnchor): string => {
  const base = `/analyses/${encodeURIComponent(id)}`;
  switch (anchor.kind) {
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

/** Provides the chat to the review's screens and draws it (or its launcher) over them. */
export function AskChatProvider({ analysisId, children }: { analysisId: AnalysisId; children: ReactNode }) {
  const [place, setPlaceState] = useState<AskAnchor | null>(null);
  const actions = useRef<ChatActions>({});
  const setPlace = useCallback((next: AskAnchor | null) => setPlaceState(next), []);
  const setActions = useCallback((next: ChatActions) => {
    actions.current = next;
  }, []);
  const navigate = useNavigate();
  const me = useMe();
  const login = me.data?.user?.login ?? "anonymous";
  const tour = useTour(analysisId);
  const questions = useQuestions(analysisId);
  const stream = useQuestionEvents(analysisId);
  const ask = useAskInChat(analysisId);
  const all = (questions.data ?? []) as AskQuestion[];

  const layoutKey = `diffpilot.askChat.${login}`;
  const draftKey = `diffpilot.askChat.draft.${login}.${analysisId}`;
  const seenKey = `diffpilot.askChat.seen.${login}.${analysisId}`;
  const [remembered, setRemembered] = useState(() => read(local, layoutKey, FIRST));
  const [draft, setDraft] = useState(() => read(session, draftKey, { text: "" }).text);
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
  const control: AskChatControl = { place, setPlace, open, setActions };

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
      document.documentElement.style.setProperty(
        "--ask-chat-pad",
        remembered.open && inspectorHidden ? `${next.height + 24}px` : "0px",
      );
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
    };
  }, [remembered]);

  const pending = usePendingReview(analysisId);
  const commenting = pending.data?.commenting_available === true;
  if (!tour.data) return <Context.Provider value={control}>{children}</Context.Provider>;
  const pr = tour.data.analysis.pr;
  const send = (body: { text: string; anchor: AskAnchor; parent_id?: QuestionId }) => ask.mutateAsync(body);
  const busy = all.some((q) => q.state === "answering");
  return (
    <Context.Provider value={control}>
      {children}
      <p className="sr-only" aria-live="polite">
        {announcement}
      </p>
      {remembered.open ? (
        <AskChat
          tour={tour.data}
          threads={threadsOf(all.filter((q) => q.mine !== false))}
          subject={subject}
          following={!wholePr}
          placeWhat={place ? subjectOf(place, tour.data).what : null}
          draft={draft}
          onDraft={(text) => {
            setDraft(text);
            write(session, draftKey, text ? { text } : null);
          }}
          onSend={(text) => send({ text, anchor: subject })}
          onReply={(root, text) => send({ text, anchor: root.anchor, parent_id: root.id })}
          onAskAgain={(q) =>
            send({
              text: q.text,
              anchor: q.anchor,
              ...(q.parent_id ? { parent_id: q.parent_id } : { parent_id: q.id }),
            })
          }
          onUnanchor={() => setWholePr(true)}
          onUsePlace={() => {
            setWholePr(false);
            setChosen(null);
          }}
          onGoto={(anchor) => void navigate(linkOf(analysisId, anchor))}
          onEvidence={(evidence) =>
            actions.current.onEvidence
              ? actions.current.onEvidence(evidence)
              : void navigate(
                  linkOf(analysisId, {
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
          about={`your private chat about PR #${pr.number}`}
          unread={marked}
          focusThread={focusThread}
          inputRef={field}
          notice={
            stream.lost
              ? "The connection that delivers answers was lost. Reconnecting; answers already given are read again."
              : null
          }
        />
      ) : (
        <div ref={launcher}>
          <AskLauncher unread={unread.length} busy={busy} onOpen={() => show()} />
        </div>
      )}
    </Context.Provider>
  );
}
