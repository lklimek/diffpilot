import type { Ref } from "react";
import type { TodosState, WorkItem } from "../../../api/generated/types";
import { clockText } from "../../format/time";
import { Tooltip } from "../../tooltip/Tooltip";
import { scoreText } from "../common/format";
import { KIND_LABEL, KindIcon } from "../common/icons";
import { LlmLine } from "../common/LlmLine";
import { PluginNotes } from "../plugin-notes/PluginNotes";
import { OpenExternal, rowOpenLabel } from "../common/OpenExternal";
import type { Navigate } from "../common/navigate";
import { SizePill } from "../common/Tags";
import { ScoreBreakdown } from "../score-breakdown/ScoreBreakdown";
import { ScoreTip } from "../score-breakdown/ScoreTip";
import { CommentsInfo } from "./CommentsInfo";
import { RowMenu, type RowActions } from "./RowMenu";
import { WorkFacts, WorkTitle } from "./WorkFacts";
import { WorkJudgement, judgementDate, type WorkJudgementProps } from "./WorkJudgement";
import type { OnFilter } from "../filter-chips/FilterChips";
import { ItemAction } from "./ItemAction";

interface WorkItemRowProps extends RowActions, WorkJudgementProps {
  item: WorkItem;
  onFilter?: OnFilter;
  snapshotAt?: string | null;
  /** Score breakdown and facts shown. */
  expanded: boolean;
  onToggle: () => void;
  /** The keyboard cursor (`j`/`k`) is on this row. */
  current?: boolean;
  /** In-app href of a pull request's review (plan §F.1); built by the screen. */
  reviewHref?: string;
  onNavigate?: Navigate;
  /** Ref of the expander button (the row's focus target for `j`/`k`). */
  expanderRef?: Ref<HTMLButtonElement>;
  /** State of the MemCan TODOs in the snapshot: a TODO row kept from an earlier scan says so (plan §G). */
  todos?: TodosState;
  /** In-app href of a pull request's Comments (owner 2026-11-23); built by the screen. */
  commentsHref?: string;
  now: Date;
}

/** The mark of a TODO row MemCan could not refresh in this scan, or null. */
function notRefreshedPill(item: WorkItem, todos: TodosState | undefined, now: Date) {
  if (item.kind !== "todo" || todos?.status !== "not_refreshed") return null;
  const at = todos.failed_at ? ` at ${clockText(todos.failed_at, now)}` : "";
  const read = todos.read_at ? `, read at ${clockText(todos.read_at, now)}` : "";
  return (
    <Tooltip tip={`MemCan could not be reached in this scan; this TODO is from an earlier scan${read}.`}>
      <span tabIndex={0} className="cursor-help rounded border border-border px-1.5">
        ◷ not refreshed (MemCan unreachable{at})
      </span>
    </Tooltip>
  );
}

/** One ranked item (mockup `rowHTML`): id, title, scan-fact tags, LLM why / next, score, action, detail. */
export function WorkItemRow({
  item,
  expanded,
  onToggle,
  current = false,
  reviewHref,
  onNavigate,
  expanderRef,
  todos,
  commentsHref,
  onWatch,
  watched,
  watching,
  onFilter,
  snapshotAt,
  onRejudge,
  rejudging,
  usageHref,
  onSnooze,
  snoozing = false,
  now,
}: WorkItemRowProps) {
  const detailId = `detail-${item.key}`;
  const llm = item.llm;
  const marker = (
    <WorkJudgement
      item={item}
      onRejudge={onRejudge}
      rejudging={rejudging}
      usageHref={usageHref}
      onNavigate={onNavigate}
    />
  );
  return (
    <li
      data-item={item.key}
      data-current={current || undefined}
      className={`border-t border-border first:border-t-0 ${current ? "shadow-[inset_4px_0_0_var(--color-accent)]" : ""}`}
    >
      <div className="grid grid-cols-[28px_minmax(0,1fr)] sm:grid-cols-[28px_108px_minmax(0,1fr)_auto] items-start gap-x-3 gap-y-1 py-2.5 pr-3 pl-2 @min-[1600px]:grid-cols-[28px_108px_minmax(380px,1.15fr)_minmax(300px,1fr)_minmax(260px,0.8fr)_auto]">
        <Tooltip tip="Show the score breakdown and facts">
          <button
            ref={expanderRef}
            type="button"
            aria-expanded={expanded}
            aria-controls={detailId}
            aria-label={`Details of ${item.id}`}
            onClick={onToggle}
            className="h-[26px] w-[26px] cursor-pointer rounded border border-transparent p-0 text-[13px] text-muted hover:border-border hover:text-text aria-expanded:text-accent"
          >
            {expanded ? "▾" : "▸"}
          </button>
        </Tooltip>
        <div className="flex flex-col gap-1.5 pt-0.5 font-mono text-[12.5px] font-semibold">
          <span className="flex items-center gap-1 font-sans text-[10.5px] tracking-wide text-muted uppercase">
            <KindIcon kind={item.kind} />
            {KIND_LABEL[item.kind] ?? item.kind}
            <span className="font-sans tracking-normal normal-case">
              <SizePill mark={item.change_size} onFilter={onFilter} />
            </span>
          </span>
          <span className="flex items-start gap-1.5">
            <span className="min-w-0 break-all">{item.id}</span>
            <OpenExternal href={item.url} label={rowOpenLabel(item)} />
          </span>
        </div>
        <div className="col-start-2 min-w-0 sm:col-start-3">
          <div className="text-sm font-semibold [overflow-wrap:anywhere]">
            <WorkTitle item={item} href={reviewHref} onNavigate={onNavigate} />
          </div>
          <div className="mt-0.5 flex flex-wrap items-center gap-1.5 text-xs text-muted">
            <WorkFacts item={item} onFilter={onFilter} snapshotAt={snapshotAt} now={now} />
            {notRefreshedPill(item, todos, now)}
            {item.kind === "pr" && (
              <CommentsInfo item={item} href={commentsHref} onNavigate={onNavigate} watched={watched} />
            )}
          </div>
          <PluginNotes notes={item.plugin_notes} itemId={item.id} full={expanded} now={now} />
        </div>
        {llm ? (
          <>
            <LlmLine
              text={llm.why}
              marker={marker}
              judgementDate={judgementDate(item)}
              className="col-start-2 sm:col-span-2 sm:col-start-3 @min-[1600px]:col-span-1 @min-[1600px]:col-start-4 @min-[1600px]:row-start-1"
            />
            <LlmLine
              text={llm.next}
              label="Next"
              judgementDate={judgementDate(item)}
              className="col-start-2 sm:col-span-2 sm:col-start-3 @min-[1600px]:col-span-1 @min-[1600px]:col-start-5 @min-[1600px]:row-start-1"
            />
          </>
        ) : (
          <p className="col-start-2 sm:col-span-2 sm:col-start-3 m-0 text-[12.5px] text-muted italic @min-[1600px]:col-span-2 @min-[1600px]:col-start-4 @min-[1600px]:row-start-1">
            {marker}
          </p>
        )}
        <div className="col-start-2 flex flex-wrap items-center justify-end gap-2 pt-px sm:col-start-4 sm:row-start-1 @min-[1600px]:col-start-6">
          <ScoreButton item={item} expanded={expanded} onToggle={onToggle} controls={detailId} />
          <ItemAction
            item={item}
            reviewHref={reviewHref}
            commentsHref={commentsHref}
            onNavigate={onNavigate}
          />
          <RowMenu
            item={item}
            onWatch={onWatch}
            watched={watched}
            watching={watching}
            onSnooze={onSnooze}
            snoozing={snoozing}
          />
        </div>
      </div>
      <div
        id={detailId}
        hidden={!expanded}
        className="border-t border-dashed border-border bg-surface-2 px-4 pt-1 pb-3.5 @min-[1100px]:pl-12"
      >
        {expanded && <ScoreBreakdown item={item} now={now} />}
      </div>
    </li>
  );
}

/** The points of an item; a click shows or hides its score breakdown (`controls` is the breakdown's id). */
export function ScoreButton({
  item,
  expanded,
  onToggle,
  controls,
}: {
  item: WorkItem;
  expanded: boolean;
  onToggle: () => void;
  controls: string;
}) {
  const provisional = item.score.state !== "final";
  const label = `Score ${provisional ? "provisional " : ""}${Math.round(item.score.total_tenths / 10)}, ${
    expanded ? "hide" : "show"
  } breakdown`;
  return (
    <Tooltip tip={<ScoreTip item={item} />}>
      <button
        type="button"
        aria-label={label}
        aria-controls={controls}
        aria-expanded={expanded}
        onClick={onToggle}
        data-score
        className={`inline-flex h-7 min-w-[46px] cursor-pointer items-center justify-center rounded-md border px-1.5 font-mono text-sm font-bold tabular-nums hover:border-accent ${
          provisional
            ? "border-dashed border-med-bd bg-transparent text-med"
            : "border-border bg-surface-2 text-text"
        }`}
      >
        {scoreText(item.score)}
      </button>
    </Tooltip>
  );
}
