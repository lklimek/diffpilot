import { useState, type RefObject } from "react";
import type { CommentItem, DecisionKind, QueueEntry } from "../../api/generated/types";
import { MenuButton, MenuItem } from "../menu/MenuButton";
import { clockText } from "../format/time";
import { CommentTitle, BotHeading } from "./CommentTitle";
import { GithubDecision, githubDecisionEvidence } from "./GithubDecision";
import { DuplicateOfLink } from "./DuplicateLinks";
import type { Navigate } from "../dashboard/common/navigate";
import {
  commentStatus,
  duplicatePrimary,
  followsPrimary,
  needsYouCount,
  needsYouQueue,
  NOT_ASSESSED,
  QUEUE_STATUS,
  queueStatusLabel,
  type QueueStatus,
} from "./queueStatus";
import { buttonClass } from "../button/button";
import { OpenExternal } from "../dashboard/common/OpenExternal";
import { ProvenanceTag, provenanceNote, type ProvenanceSource } from "../section/ProvenanceTag";
import { Tooltip } from "../tooltip/Tooltip";
import { CHOICE_LABEL, decisionLabel, markText } from "./decisions";
import { itemId } from "./parts";

/** The key of a queue entry (its item numbers). */
export const entryKey = (entry: QueueEntry) => entry.item_nos.join("+");

/** The queue in the list's order: the questions of Needs you, then the PR-level questions (owner 2026-11-23). */
export const inListOrder = (queue: QueueEntry[]): QueueEntry[] => [
  ...queue.filter((e) => e.reason !== "pr_question"),
  ...queue.filter((e) => e.reason === "pr_question"),
];

/**
 * Where Decide and Skip go next (plan §3.2 item 9): the items of the entries after `entry`, then those from the top,
 * without `entry`'s own; the whole queue when `entry` is not in it (an item opened by link).
 */
export function itemsAfter(queue: QueueEntry[], entry: QueueEntry): number[] {
  const at = queue.findIndex((e) => entryKey(e) === entryKey(entry));
  const order = at < 0 ? queue : [...queue.slice(at + 1), ...queue.slice(0, at)];
  return order.flatMap((e) => e.item_nos).filter((no) => !entry.item_nos.includes(no));
}

interface QueueListProps {
  queue: QueueEntry[];
  items: ReadonlyMap<number, CommentItem>;
  status?: QueueStatus;
  onStatus?: (status: QueueStatus) => void;
  statusHost?: RefObject<HTMLDivElement | null>;
  inPass?: number;
  since?: string | null;
  now?: Date;
  /** The entry shown. */
  current: string | null;
  onOpen: (entry: QueueEntry) => void;
  itemHref?: (no: number) => string;
  onNavigate?: Navigate;
  /** Items "Decide the rest" would decide; the button is hidden without any. */
  restCount: number;
  onDecideRest: () => void;
  /** Opens the decision log. */
  onOpenLog: () => void;
  /** Opens carry-overs. */
  onOpenCarryOvers: () => void;
}

const ROW =
  "flex min-h-6 w-full cursor-pointer items-start gap-1.5 rounded-md border-0 bg-transparent px-2 py-1 text-left text-text hover:bg-surface-2 aria-[current=true]:bg-accent-bg aria-[current=true]:font-semibold";

const MARK = "inline-flex max-w-full items-center gap-1 border px-1.5 text-[11px]";

// An LLM recommendation's colour by what it asks of you: change code here, nothing to change, a reply, elsewhere.
// Never the green of a fact badge; dashed and tagged LLM; the decision is always written out (plan §3.9).
const RECOMMENDATION_LOOK: Partial<Record<DecisionKind, string>> = {
  fix_here: "border-med-bd bg-med-bg text-med",
  resolve: "border-chg-bd bg-chg-bg text-chg",
  reject: "border-mv-bd bg-mv-bg text-mv",
  handled_in_pr: "border-low-bd bg-low-bg text-low",
  follow_up_pr: "border-low-bd bg-low-bg text-low",
  upstream_issue: "border-low-bd bg-low-bg text-low",
  todo: "border-low-bd bg-low-bg text-low",
};

/**
 * The state marks of a Needs you row, in words with colour (owner 2026-11-23): why it is asked, its gates (tagged LLM
 * when the assessment named them), the reviewer's severity words (tagged GitHub) and the LLM's recommendation.
 */
function Marks({ entry, first }: { entry: QueueEntry; first: CommentItem }) {
  const group = entry.item_nos.length > 1;
  const rec = first.assessment?.recommendation;
  const urgent = !group && (first.gates.length > 0 || first.bot_severity === "Blocking");
  const marks = [
    entry.reason === "asked_again" && (
      <span
        key="reason"
        data-mark="reason"
        className={`${MARK} rounded-full ${urgent ? "border-border text-muted" : "border-med-bd bg-med-bg text-med"}`}
      >
        asked again
      </span>
    ),
    entry.reason === "answered" && (
      <span key="reason" data-mark="reason" className={`${MARK} rounded-full border-border text-text`}>
        Ask answer
      </span>
    ),
    ...(group ? [] : first.gates).map((g) => (
      <span
        key={`gate-${g.gate}`}
        data-mark="gate"
        data-gate={g.gate}
        className={`${MARK} rounded-full border-high-bd bg-high-bg font-semibold text-high ${
          g.source === "assessment" ? "border-dashed" : ""
        }`}
      >
        Gate {g.gate}
        {g.source === "assessment" && <ProvenanceTag source="llm" tip={false} />}
      </span>
    )),
    !group && first.bot_severity && (
      <span
        key="marker"
        data-mark="marker"
        className={`${MARK} rounded-full bg-surface ${
          first.bot_severity === "Blocking" && !first.gates.length
            ? "border-high-bd font-semibold text-high"
            : "border-border text-text"
        }`}
      >
        <ProvenanceTag source="github" tip={false} /> {first.bot_severity}
      </span>
    ),
    rec && (
      <span
        key="recommendation"
        data-mark="recommendation"
        data-recommendation={rec.kind}
        className={`${MARK} rounded border-dashed ${(!urgent && entry.reason !== "asked_again" && RECOMMENDATION_LOOK[rec.kind]) || "border-border text-text"}`}
      >
        <ProvenanceTag source="llm" tip={false} /> recommends {CHOICE_LABEL[rec.kind] ?? rec.kind}
      </span>
    ),
  ].filter(Boolean);
  if (marks.length === 0) return null;
  return (
    <span data-marks className="mt-0.5 flex flex-wrap items-center gap-1">
      {marks.flatMap((mark, i) => (i === 0 ? [mark] : [" ", mark]))}
    </span>
  );
}

/** The sources of the tags among a row's marks ({@link Marks}), GitHub first. */
function tagSources(entry: QueueEntry, first: CommentItem): ProvenanceSource[] {
  const group = entry.item_nos.length > 1;
  const github = !group && (first.bot_severity || first.imported_decision || first.state === "outside");
  const llm =
    Boolean(first.llm_title) ||
    first.duplicate_of != null ||
    first.assessment?.recommendation !== undefined ||
    (!group && first.gates.some((g) => g.source === "assessment"));
  return [...(github ? ["github" as const] : []), ...(llm ? ["llm" as const] : [])];
}

/**
 * The row's tooltip: what a click opens and what its marks' tags mean (the marks are not focusable inside the
 * row's button, plan §3.9).
 */
const rowTip = (entry: QueueEntry, first: CommentItem) =>
  [
    entry.item_nos.length > 1 ? "Opens these questions as one group." : "Opens this question.",
    ...tagSources(entry, first).map(provenanceNote),
  ].join(" ");

const DESCRIPTION: Partial<Record<QueueStatus, string>> = {
  needs_you: "Undecided questions that need you. PR-level questions are last.",
  assessing: "In the current assessment pass.",
  waiting: "Read from GitHub and waiting for the next pass.",
  not_assessed: "Assessment did not complete or reached the daily limit.",
  decided: "Decisions made here, carried from earlier rounds or imported from GitHub.",
  not_a_review_point: "Items marked in diffpilot.",
  duplicates: "Items whose decision follows their primary.",
  status_comments: "Bot status and summaries.",
};

export function commentStatusText(item: CommentItem, inPass?: number, since?: string | null, now?: Date) {
  switch (commentStatus(item)) {
    case "assessing":
      if (inPass === undefined) return `◌ ${queueStatusLabel("assessing", inPass).toLowerCase()}`;
      return since ? `◔ assessing since ${clockText(since, now)}` : "◔ assessing now";
    case "waiting":
      return "◌ waiting for assessment";
    case "not_assessed":
      return item.assessment_state.state === "not_assessed"
        ? `Not assessed: ${NOT_ASSESSED[item.assessment_state.reason] ?? item.assessment_state.reason}`
        : "Not assessed";
    case "not_a_review_point":
      return markText(item.not_a_review_point!, now);
    case "duplicates":
      return `⧉ Same point as C${item.duplicate_of}`;
    case "status_comments":
      return "Status comment";
    default:
      return null;
  }
}

/** Pick a status, then open an item or a recommendation group within that view. */
export function QueueList({
  queue,
  items,
  current,
  onOpen,
  itemHref,
  onNavigate,
  restCount,
  onDecideRest,
  onOpenLog,
  onOpenCarryOvers,
  status: controlledStatus,
  onStatus,
  statusHost,
  inPass,
  since,
  now,
}: QueueListProps) {
  const [localStatus, setLocalStatus] = useState<QueueStatus>("needs_you");
  const status = controlledStatus ?? localStatus;
  const all = [...items.values()];
  const questions = inListOrder(needsYouQueue(queue, items));
  const needs = questions.flatMap((entry) => entry.item_nos);
  const gates = needs.filter((no) => items.get(no)!.gates.length).length;
  const subsets = {
    needs_you: questions,
    ...(Object.fromEntries(
      Object.keys(QUEUE_STATUS)
        .filter((key) => key !== "needs_you")
        .map((key) => [
          key,
          all
            .filter((item) => key === "all" || commentStatus(item) === key)
            .map((item): QueueEntry => ({
              item_nos: [item.no],
              reason: item.kind === "pr_question" ? "pr_question" : "single",
            })),
        ]),
    ) as Record<Exclude<QueueStatus, "needs_you">, QueueEntry[]>),
  };
  const count = (key: QueueStatus) =>
    key === "needs_you" ? needsYouCount(0, queue, items) : subsets[key].length;
  const selected = subsets[status];
  const visible =
    status === "all"
      ? selected.filter((entry) => {
          const item = items.get(entry.item_nos[0]!)!;
          const primary = duplicatePrimary(item);
          return !followsPrimary(item) || primary == null || !items.has(primary);
        })
      : selected;

  const row = (entry: QueueEntry) => {
    const first = items.get(entry.item_nos[0]!);
    if (!first) return null;
    const group = entry.item_nos.length > 1;
    const rec = first.assessment?.recommendation;
    const duplicates = all.filter(
      (item) =>
        followsPrimary(item) && item.duplicate_of != null && entry.item_nos.includes(item.duplicate_of),
    );
    const oneClass = entry.item_nos.every(
      (no) => items.get(no)?.assessment?.recommendation.reason_class === rec?.reason_class,
    );
    const title =
      group && rec ? (
        `${CHOICE_LABEL[rec.kind]}${oneClass ? `: ${rec.reason_class.replaceAll("_", " ")}` : ""} (${entry.item_nos.length})`
      ) : (
        <CommentTitle item={first} tip={false} />
      );
    const imported = first.imported_decision || first.state === "outside";
    const previous = first.decisions.at(-1);
    const state = commentStatus(first);
    const note = imported ? (
      <GithubDecision item={first} showEvidence={false} tip={false} />
    ) : state === "decided" && previous ? (
      `${decisionLabel(previous)} · ${first.repeat_of != null ? `carried from C${first.repeat_of}` : clockText(previous.decided_at, now)}`
    ) : (
      commentStatusText(first, inPass, since, now)
    );
    const evidence = imported ? githubDecisionEvidence(first) : null;
    const primary = duplicatePrimary(first);
    return (
      <li key={entryKey(entry)}>
        <div className="flex items-center gap-0.5">
          <Tooltip tip={rowTip(entry, first)} placement="right" className="min-w-0 flex-1">
            <button
              type="button"
              aria-current={current === entryKey(entry)}
              onClick={() => onOpen(entry)}
              className={ROW}
            >
              <span className="min-w-8 font-mono text-xs text-muted">{group ? "G" : itemId(first)}</span>{" "}
              <span className="min-w-0 flex-1">
                <span className="block wrap-anywhere">{title}</span>
                {!group && <BotHeading item={first} />}
                {note && (
                  <span className="block text-xs font-normal text-muted">
                    {note}
                    {state === "duplicates" && (
                      <>
                        {" "}
                        <ProvenanceTag source="llm" tip={false} />
                      </>
                    )}
                  </span>
                )}
                {state === "needs_you" && needs.includes(first.no) && <Marks entry={entry} first={first} />}
              </span>
            </button>
          </Tooltip>
          {!group && (
            <OpenExternal
              href={evidence ?? first.url}
              label={evidence ? `Evidence for ${itemId(first)} on GitHub` : `Open ${itemId(first)} on GitHub`}
            />
          )}
        </div>
        {!group && primary != null && !followsPrimary(first) && (
          <div className="ml-9">
            <DuplicateOfLink
              item={first}
              href={items.has(primary) ? itemHref?.(primary) : undefined}
              onNavigate={onNavigate}
            />
          </div>
        )}
        {duplicates.length > 0 && status !== "duplicates" && (
          <details className="ml-9 text-xs text-muted">
            <summary className="min-h-6 cursor-pointer content-center">
              ⧉ +{duplicates.length} same point <ProvenanceTag source="llm" tip={false} />
            </summary>
            <ul className="m-0 list-none px-2">
              {duplicates.map((item) => (
                <li key={item.id} className="flex flex-wrap items-center gap-x-1">
                  <Tooltip tip={`Open duplicate ${itemId(item)}.`}>
                    <button
                      type="button"
                      className="min-h-6 cursor-pointer text-left text-accent underline"
                      onClick={() => onOpen({ item_nos: [item.no], reason: "single" })}
                    >
                      {itemId(item)} · {item.author}
                    </button>
                  </Tooltip>
                  <OpenExternal href={item.url} label={`Open ${itemId(item)} on GitHub`} />
                </li>
              ))}
            </ul>
          </details>
        )}
      </li>
    );
  };
  const countedLabel = (key: QueueStatus) => {
    const label = queueStatusLabel(key, inPass);
    return key === "not_a_review_point" ? `${label} (${count(key)})` : `${label} · ${count(key)}`;
  };
  const trigger = countedLabel(status);
  const gateText = status === "needs_you" && gates ? `${gates} ${gates === 1 ? "gate" : "gates"}` : null;
  return (
    <nav aria-label="Queue" className="min-w-0 rounded-lg border border-border bg-surface px-3 py-3">
      <div ref={statusHost} data-queue-status className="mb-2">
        <MenuButton
          label={`Status: ${trigger}${gateText ? `, ${gateText}` : ""}`}
          trigger={
            <>
              {trigger}
              {gateText && <span className="ml-1 font-semibold text-high"> · ▲ {gateText}</span>}{" "}
              <span aria-hidden>▾</span>
            </>
          }
        >
          {(Object.keys(QUEUE_STATUS) as QueueStatus[]).map((key) => (
            <MenuItem
              key={key}
              label={countedLabel(key)}
              selection={{ kind: "radio", checked: key === status }}
              description={
                key === "assessing" && inPass === undefined
                  ? "Waiting for assessment or in an assessment pass."
                  : DESCRIPTION[key]
              }
              onSelect={() => (onStatus ?? setLocalStatus)(key)}
            />
          ))}
        </MenuButton>
      </div>
      {visible.length ? (
        <ol className="m-0 list-none p-0">{visible.map(row)}</ol>
      ) : (
        <p className="m-0 px-2 text-xs text-muted">No items in this view.</p>
      )}
      {status === "needs_you" && restCount > 0 && (
        <Tooltip tip="Each recommendation, or a GitHub proposal where there is one, becomes its own decision after a confirmation list; gate items are left out.">
          <button
            type="button"
            onClick={onDecideRest}
            className={`${buttonClass("plain", true)} my-2 text-left whitespace-normal!`}
          >
            Decide the rest ({restCount})
          </button>
        </Tooltip>
      )}
      {status === "decided" && (
        <div className="flex flex-wrap gap-2 text-xs">
          <Tooltip tip="Opens the decision log with the agents’ reports.">
            <button
              type="button"
              onClick={onOpenLog}
              className="min-h-6 cursor-pointer text-accent underline"
            >
              Decision log
            </button>
          </Tooltip>
          <Tooltip tip="Opens the carry-overs of this pull request.">
            <button
              type="button"
              onClick={onOpenCarryOvers}
              className="min-h-6 cursor-pointer text-accent underline"
            >
              Carry-overs
            </button>
          </Tooltip>
        </div>
      )}
    </nav>
  );
}
