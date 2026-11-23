import type { CommentItem, QueueEntry } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { OpenExternal } from "../dashboard/common/OpenExternal";
import { Tooltip } from "../tooltip/Tooltip";
import { CHOICE_LABEL, decisionLabel } from "./decisions";
import { GateChips } from "./QueueCards";
import { itemId } from "./parts";

/** The key of a queue entry (its item numbers). */
export const entryKey = (entry: QueueEntry) => entry.item_nos.join("+");

interface QueueListProps {
  queue: QueueEntry[];
  items: ReadonlyMap<number, CommentItem>;
  /** The entry shown. */
  current: string | null;
  onOpen: (entry: QueueEntry) => void;
  /** Items "Decide the rest" would decide; the button is hidden without any. */
  restCount: number;
  onDecideRest: () => void;
  /** Opens the decision log. */
  onOpenLog: () => void;
  /** Opens carry-overs. */
  onOpenCarryOvers: () => void;
}

const ROW =
  "flex min-h-6 w-full cursor-pointer items-center gap-1.5 rounded-md border-0 bg-transparent px-2 py-1 text-left text-text hover:bg-surface-2 aria-[current=true]:bg-accent-bg aria-[current=true]:font-semibold";

/** The Questions queue (mockup left column): PR-level, questions, Ask answers, carried, decided, outside. */
export function QueueList({
  queue,
  items,
  current,
  onOpen,
  restCount,
  onDecideRest,
  onOpenLog,
  onOpenCarryOvers,
}: QueueListProps) {
  const all = [...items.values()];
  const prLevel = queue.filter((e) => e.reason === "pr_question");
  const questions = queue.filter((e) => e.reason !== "pr_question");
  const carried = all.filter(
    (i) => i.state === "repeat" || (i.decisions.at(-1)?.via === "carried" && i.state !== "reopened"),
  );
  const decided = all.filter((i) => ["decided", "executing", "done"].includes(i.state));
  const outside = all.filter((i) => i.state === "outside");
  const row = (entry: QueueEntry) => {
    const first = items.get(entry.item_nos[0]!);
    if (!first) return null;
    const group = entry.item_nos.length > 1;
    const rec = first.assessment?.recommendation;
    // Resolved review-body items form one group across reason classes (plan §2.4).
    const oneClass = entry.item_nos.every(
      (no) => items.get(no)?.assessment?.recommendation.reason_class === rec?.reason_class,
    );
    return (
      <li key={entryKey(entry)} className="flex items-center gap-0.5">
        <button
          type="button"
          aria-current={current === entryKey(entry)}
          onClick={() => onOpen(entry)}
          className={ROW}
        >
          <span className="min-w-8 font-mono text-xs text-muted">{group ? "G" : itemId(first)}</span>
          <span className="min-w-0 flex-1 truncate">
            {group && rec
              ? `${CHOICE_LABEL[rec.kind]}${oneClass ? `: ${rec.reason_class.replaceAll("_", " ")}` : ""} (${entry.item_nos.length})`
              : first.title}
          </span>
          {entry.reason === "asked_again" && (
            <span className="rounded-full border border-med-bd bg-med-bg px-1.5 text-[11px] text-med">
              asked again
            </span>
          )}
          {entry.reason === "answered" && (
            <span className="rounded-full border border-accent px-1.5 text-[11px] text-accent">
              Ask answer
            </span>
          )}
          {!group && <GateChips item={first} />}
        </button>
        {!group && <OpenExternal href={first.url} label={`Open ${itemId(first)} on GitHub`} />}
      </li>
    );
  };
  const h = "mt-3 mb-1 text-xs font-bold tracking-wide text-muted uppercase first:mt-0";
  return (
    <nav aria-label="Queue" className="rounded-lg border border-border bg-surface px-3 py-3">
      {prLevel.length > 0 && (
        <>
          <h3 className={h}>PR-level ({prLevel.length})</h3>
          <ol className="m-0 list-none p-0">{prLevel.map(row)}</ol>
        </>
      )}
      <h3 className={h}>Needs you ({questions.reduce((n, e) => n + e.item_nos.length, 0)})</h3>
      {questions.length ? (
        <ol className="m-0 list-none p-0">{questions.map(row)}</ol>
      ) : (
        <p className="m-0 px-2 text-xs text-muted">none</p>
      )}
      {restCount > 0 && (
        <Tooltip tip="Each recommendation becomes its own decision after a confirmation list; gate items are left out.">
          <button
            type="button"
            onClick={onDecideRest}
            className={`${buttonClass("plain", true)} my-2 text-left whitespace-normal!`}
          >
            Decide the rest per recommendation ({restCount})
          </button>
        </Tooltip>
      )}
      <h3 className={h}>Previously decided ({carried.length + decided.length})</h3>
      <ul className="m-0 list-none p-0 text-xs">
        {carried.map((i) => (
          <li key={i.id} className="px-2 py-0.5">
            <Tooltip tip="Opens the carry-overs of this pull request.">
              <button
                type="button"
                onClick={onOpenCarryOvers}
                className="cursor-pointer border-0 bg-transparent p-0 text-left text-accent underline"
              >
                {itemId(i)} repeat of C{i.repeat_of} ·{" "}
                {i.decisions.at(-1) ? decisionLabel(i.decisions.at(-1)!) : ""}
              </button>
            </Tooltip>
          </li>
        ))}
      </ul>
      <p className="m-0 px-2 text-xs text-muted">
        {decided.length} decided here · {decided.filter((i) => i.state === "executing").length} with effects
        open ·{" "}
        <Tooltip tip="Opens the decision log with the agents' reports.">
          <button
            type="button"
            onClick={onOpenLog}
            className="cursor-pointer border-0 bg-transparent p-0 text-accent underline"
          >
            decision log
          </button>
        </Tooltip>
      </p>
      <h3 className={h}>Outside diffpilot ({outside.length})</h3>
      {outside.map((i) => (
        <p key={i.id} className="m-0 px-2 text-xs text-muted">
          {itemId(i)} resolved on GitHub by {i.outside?.by ?? i.github.resolved_by}, not through diffpilot
        </p>
      ))}
    </nav>
  );
}
