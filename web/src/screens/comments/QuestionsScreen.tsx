import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { ApiError } from "../../api/client";
import type { CommentItem, DecideRequest, DecisionBatchRow, QueueEntry } from "../../api/generated/types";
import {
  useCommentContext,
  useCommentLog,
  useDecide,
  useDecideBatch,
  useItemAction,
} from "../../api/review-loop";
import { CollapsibleSection } from "../../components/collapsible/CollapsibleSection";
import { isTypingField } from "../../components/comments/DecisionForm";
import { itemId } from "../../components/comments/parts";
import { assessmentText } from "../../components/comments/ProcessCommentsCard";
import { QuestionCard, type ContextState } from "../../components/comments/QuestionCard";
import {
  AssessmentRunningCard,
  DecideRestCard,
  EmptyQueueCard,
  GroupCard,
  PrQuestionCard,
} from "../../components/comments/QueueCards";
import { entryKey, QueueList } from "../../components/comments/QueueList";
import { Kbd } from "../../components/help/Kbd";
import { Tooltip } from "../../components/tooltip/Tooltip";
import { buttonClass } from "../../components/button/button";
import { useAskChat, useAskPlace } from "../review/askChat";
import { commentsPath, prPath } from "../pr/paths";
import { useComments } from "./CommentsRoute";

const errorText = (error: unknown) =>
  error instanceof ApiError ? error.message : "Something went wrong. Try again.";

const LINES_STEP = 12;
const KEYS: [string, string][] = [
  ["j / k", "next / previous question"],
  ["1–8", "choose an option"],
  ["Enter", "decide"],
  ["i", "discuss a group row individually"],
  ["a", "Ask about this comment"],
  ["s", "skip for now"],
  ["g", "open the comment on GitHub"],
  ["t", "table page"],
  ["?", "this help"],
  ["Esc", "close"],
];

/** V1 Questions (plan §3.2, §2.4): one question at a time, groups, "Decide the rest", PR-level questions. */
export function QuestionsScreen() {
  const { pr, overview, items, askAnswers, login, now } = useComments();
  const [search, setSearch] = useSearchParams();
  const navigate = useNavigate();
  const chat = useAskChat();
  const decide = useDecide(pr);
  const batch = useDecideBatch(pr);
  const action = useItemAction(pr);
  const log = useCommentLog(pr);
  const [skipped, setSkipped] = useState<ReadonlySet<string>>(new Set());
  const [individual, setIndividual] = useState<ReadonlySet<number>>(new Set());
  const [rest, setRest] = useState(false);
  const [help, setHelp] = useState(false);
  const [announcement, setAnnouncement] = useState("");
  // The last single decision, offered for Undo until one of its effects is claimed (plan §3.2 item 9).
  const [undoable, setUndoable] = useState<{ no: number } | null>(null);
  const [lines, setLines] = useState<{ item: number | null; from: number | null; to: number | null }>({
    item: null,
    from: null,
    to: null,
  });
  const byNo = useMemo(() => new Map(items.map((i) => [i.no, i])), [items]);
  const storageKey = `${login}:comments-question`;

  // The queue with groups split where the owner chose to discuss rows individually; skipped entries last.
  const queue = useMemo(() => {
    const out: QueueEntry[] = [];
    for (const entry of overview.queue) {
      const nos = entry.item_nos.filter((n) => byNo.has(n));
      const singles = nos.filter((n) => individual.has(n));
      const kept = nos.filter((n) => !individual.has(n));
      out.push(...singles.map((n) => ({ item_nos: [n], reason: "single" as const })));
      if (kept.length >= 2) out.push({ ...entry, item_nos: kept });
      else if (kept.length === 1)
        out.push({ ...entry, item_nos: kept, reason: nos.length > 1 ? "single" : entry.reason });
    }
    return [...out.filter((e) => !skipped.has(entryKey(e))), ...out.filter((e) => skipped.has(entryKey(e)))];
  }, [overview.queue, byNo, individual, skipped]);

  const asked = search.get("item");
  const askedNo = asked && /^[CP]\d+$/.test(asked) ? Number(asked.slice(1)) : null;
  const askedEntry = askedNo === null ? undefined : queue.find((e) => e.item_nos.includes(askedNo));
  const entry: QueueEntry | null =
    askedEntry ??
    (askedNo !== null && byNo.has(askedNo) ? { item_nos: [askedNo], reason: "single" } : (queue[0] ?? null));
  const index = entry ? queue.findIndex((e) => entryKey(e) === entryKey(entry)) : -1;
  const single = entry && entry.item_nos.length === 1 ? byNo.get(entry.item_nos[0]!) : undefined;

  const open = (next: QueueEntry | undefined) => {
    setRest(false);
    if (!next) return setSearch({}, { replace: true });
    const first = byNo.get(next.item_nos[0]!);
    if (first) setSearch({ item: itemId(first) });
  };

  const contextItem = single && single.location ? single.no : null;
  useAskPlace(single && single.kind !== "pr_question" ? { kind: "comment_item", item: single.id } : null);
  const range = lines.item === contextItem ? lines : { from: null, to: null };
  const context = useCommentContext(pr, contextItem, { from: range.from, to: range.to });
  const contextState: ContextState = !contextItem
    ? { state: "none" }
    : context.isError
      ? { state: "error", message: errorText(context.error) }
      : context.data
        ? { state: "ready", value: context.data }
        : { state: "loading" };

  const afterDecided = (what: string, count: number) => {
    const left = Math.max(0, overview.counts.undecided - count);
    setAnnouncement(`${what}: decision recorded. ${left} ${left === 1 ? "question" : "questions"} left.`);
    setSearch({}, { replace: true });
    setSkipped(new Set());
  };
  const onDecide = (item: CommentItem, request: DecideRequest) =>
    decide.mutate(
      { no: item.no, body: request },
      {
        onSuccess: () => {
          afterDecided(itemId(item), 1);
          setUndoable({ no: item.no });
        },
      },
    );
  const onBatch = (via: "group" | "bulk", rows: DecisionBatchRow[]) =>
    batch.mutate(
      { via, rows },
      {
        onSuccess: () => {
          afterDecided(`${rows.length} items`, rows.length);
          setUndoable(null);
        },
      },
    );
  const undo = () => {
    const item = undoable && byNo.get(undoable.no);
    if (!item) return;
    action.mutate(
      { action: "undo", no: item.no, revision: item.revision },
      {
        onSuccess: () => {
          setUndoable(null);
          setAnnouncement(`${itemId(item)}: decision undone; it is asked again.`);
          setSearch({ item: itemId(item) });
        },
      },
    );
  };
  // "Skip for now": last in this reader's queue, here at once and on the server until the item changes.
  const skip = () => {
    if (entry) setSkipped(new Set(skipped).add(entryKey(entry)));
    for (const no of entry?.item_nos ?? []) {
      const item = byNo.get(no);
      if (item) action.mutate({ action: "skip", no, revision: item.revision });
    }
    setSearch({}, { replace: true });
  };
  const ask = (item: CommentItem) => chat.open({ anchor: { kind: "comment_item", item: item.id } });

  // Queue keys (plan §3.8); the card handles 1–8, Enter, a and s.
  const latestRef = useRef({ queue, index, single, navigate, pr, open });
  useLayoutEffect(() => {
    latestRef.current = { queue, index, single, navigate, pr, open };
  });
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        event.ctrlKey ||
        event.metaKey ||
        event.altKey ||
        isTypingField(event.target)
      )
        return;
      if ((event.target as HTMLElement | null)?.closest?.("[data-ask-chat]")) return;
      const l = latestRef.current;
      if (event.key === "j" || event.key === "k") {
        event.preventDefault();
        l.open(l.queue[l.index + (event.key === "j" ? 1 : -1)]);
      } else if (event.key === "t") {
        event.preventDefault();
        void l.navigate(commentsPath(l.pr, "table"));
      } else if (event.key === "g" && l.single?.url) {
        event.preventDefault();
        window.open(l.single.url, "_blank", "noopener,noreferrer");
      } else if (event.key === "?") {
        event.preventDefault();
        setHelp((h) => !h);
      } else if (event.key === "Escape") {
        setHelp(false);
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  const reask =
    single?.state === "reopened"
      ? log.data?.carry_events.find((e) => e.item_no === single.no && e.kind === "asked_again")
      : undefined;
  const restItems = queue
    .flatMap((e) => e.item_nos)
    .map((n) => byNo.get(n)!)
    .filter((i) => i && i.kind !== "pr_question" && i.gates.length === 0 && i.assessment);
  const leftOut = queue
    .flatMap((e) => e.item_nos)
    .map((n) => byNo.get(n)!)
    .filter((i) => i && i.gates.length > 0);
  const running = overview.assessment.running;
  const error = decide.isError ? errorText(decide.error) : batch.isError ? errorText(batch.error) : null;
  const undoError = undoable && action.isError ? errorText(action.error) : null;

  let card;
  if (rest)
    card = (
      <DecideRestCard
        items={restItems}
        leftOut={leftOut}
        onConfirm={(rows) => onBatch("bulk", rows)}
        onCancel={() => setRest(false)}
        busy={batch.isPending}
        error={error}
      />
    );
  else if (!entry)
    card = running ? (
      <AssessmentRunningCard progress={overview.assessment} />
    ) : (
      <EmptyQueueCard
        read={overview.read}
        assessed={overview.assessment.assessed}
        head={overview.head}
        now={now}
      />
    );
  else if (entry.item_nos.length > 1)
    card = (
      <GroupCard
        key={entryKey(entry)}
        items={entry.item_nos.map((n) => byNo.get(n)!)}
        position={{ index: index + 1, total: queue.length }}
        onDecide={(rows) => onBatch("group", rows)}
        onDiscussIndividually={(no) => {
          setIndividual(new Set(individual).add(no));
          setAnnouncement(`C${no} will be asked on its own.`);
        }}
        onSkip={skip}
        busy={batch.isPending}
        error={error}
        storageKey={storageKey}
      />
    );
  else if (single?.kind === "pr_question")
    card = (
      <PrQuestionCard
        key={single.id}
        item={single}
        prUrl={overview.url}
        onDecide={(request) => onDecide(single, request)}
        onSkip={skip}
        busy={decide.isPending}
        error={error}
      />
    );
  else if (single?.state === "assessing") card = <AssessmentRunningCard progress={overview.assessment} />;
  else if (single)
    card = (
      <QuestionCard
        key={single.id}
        item={single}
        reask={reask}
        position={{ index: index + 1, total: Math.max(queue.length, index + 1) }}
        prNumber={pr.number}
        purpose={overview.purpose}
        context={contextState}
        flowHref={(f) =>
          `/analyses/${encodeURIComponent(context.data?.analysis ?? "")}#${encodeURIComponent(f.step)}`
        }
        filesHref={
          context.data?.analysis && context.data.analysis_head === overview.head
            ? `/analyses/${encodeURIComponent(context.data.analysis)}/files`
            : null
        }
        prPageHref={prPath(pr)}
        onNavigate={(href) => void navigate(href)}
        askAnswers={askAnswers(single)}
        onAsk={() => ask(single)}
        onOpenAnswers={() => ask(single)}
        onDecide={(request) => onDecide(single, request)}
        onSkip={skip}
        onMoreLines={(direction) => {
          const shown = context.data?.lines;
          const first = shown?.[0]?.line ?? 1;
          const last = shown?.at(-1)?.line ?? 1;
          setLines({
            item: single.no,
            from: direction === "above" ? Math.max(1, first - LINES_STEP) : first,
            to: direction === "below" ? last + LINES_STEP : last,
          });
        }}
        busy={decide.isPending}
        error={error}
        now={now}
        storageKey={storageKey}
      />
    );

  return (
    <div className="grid items-start gap-3.5 lg:grid-cols-[clamp(230px,15vw,320px)_minmax(0,1fr)_clamp(240px,18vw,380px)]">
      <QueueList
        queue={queue}
        items={byNo}
        current={rest ? null : entry && entryKey(entry)}
        onOpen={open}
        restCount={restItems.length}
        onDecideRest={() => setRest(true)}
        onOpenLog={() => void navigate(commentsPath(pr, "log"))}
        onOpenCarryOvers={() => void navigate(commentsPath(pr, "carry-overs"))}
      />
      <div className="grid min-w-0 gap-3">
        {running && entry && (
          <p role="status" className="m-0 rounded-md border border-low-bd bg-low-bg px-3 py-1.5 text-sm">
            {assessmentText(overview.assessment)}. You can already decide items assessed earlier.
          </p>
        )}
        <div className={undoable ? "flex flex-wrap items-center gap-2 text-sm" : "sr-only"}>
          <p className="m-0" aria-live="polite">
            {announcement}
          </p>
          {undoable && (
            <Tooltip tip="Takes the decision back while no external agent has claimed its effects; the comment is asked again.">
              <button
                type="button"
                aria-disabled={action.isPending || undefined}
                onClick={() => !action.isPending && undo()}
                className={buttonClass("plain", true)}
              >
                Undo
              </button>
            </Tooltip>
          )}
          {undoError && <span className="text-high">{undoError}</span>}
        </div>
        {card}
        {help && (
          <section
            aria-label="Keyboard shortcuts"
            className="rounded-lg border border-border bg-surface px-4 py-3"
          >
            <dl className="m-0 grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 text-sm">
              {KEYS.map(([k, v]) => (
                <div key={k} className="contents">
                  <dt>
                    <Kbd>{k}</Kbd>
                  </dt>
                  <dd className="m-0">{v}</dd>
                </div>
              ))}
            </dl>
          </section>
        )}
      </div>
      <aside aria-label="PR panel" className="rounded-lg border border-border bg-surface px-4 pb-3">
        <CollapsibleSection title="Counts" storageKey={`${storageKey}:counts`}>
          <table className="w-full text-[13px]">
            <tbody>
              {(
                [
                  ["Undecided questions", overview.counts.undecided],
                  ["… gate items", leftOut.length],
                  ["Asked again", overview.counts.asked_again],
                  ["Decisions waiting for agents", overview.counts.waiting_for_agents],
                  ["Failed effects", overview.counts.failed_effects],
                  ["Unresolved threads on GitHub", overview.counts.unresolved_threads],
                  ["Resolved outside diffpilot", overview.counts.outside],
                ] as const
              ).map(([label, n]) => (
                <tr key={label}>
                  <td className="py-0.5">{label}</td>
                  <td className="py-0.5 text-right tabular-nums">{n}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </CollapsibleSection>
        <p className="mt-2 mb-0 text-xs text-muted">
          Replies and resolves: external agents. Press <Kbd>?</Kbd> for the keys.
        </p>
      </aside>
    </div>
  );
}
