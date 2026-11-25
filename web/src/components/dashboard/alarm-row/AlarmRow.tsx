import type { Ref } from "react";
import type { DashboardItem } from "../../../api/provisional/dashboard";
import { Tooltip } from "../../tooltip/Tooltip";
import { LlmLine } from "../common/LlmLine";
import { OpenExternal, rowOpenLabel } from "../common/OpenExternal";
import type { Navigate } from "../common/navigate";
import { SizePill } from "../common/Tags";
import { ScoreBreakdown } from "../score-breakdown/ScoreBreakdown";
import { ItemAction } from "../work-item/ItemAction";
import { CommentsInfo } from "../work-item/CommentsInfo";
import { ScoreButton } from "../work-item/WorkItemRow";

import { RowMenu, type RowActions } from "../work-item/RowMenu";
import { WorkFacts, WorkTitle } from "../work-item/WorkFacts";
import { WorkJudgement, judgementDate, type WorkJudgementProps } from "../work-item/WorkJudgement";
import type { OnFilter } from "../filter-chips/FilterChips";

interface AlarmRowProps extends Pick<RowActions, "onWatch" | "watched" | "watching">, WorkJudgementProps {
  /** A P0 item. */
  item: DashboardItem;
  onFilter?: OnFilter;
  snapshotAt?: string | null;
  /** In-app href of a pull request's review (plan §F.1). */
  reviewHref?: string;
  /** In-app href of a pull request's Comments (owner 2026-11-23), as on ranked rows. */
  commentsHref?: string;
  onNavigate?: Navigate;
  /** The keyboard cursor is on this alarm. */
  current?: boolean;
  /** The alarm's frame, focused by `j`/`k`. */
  focusRef?: Ref<HTMLDivElement>;
  /** The score breakdown is open. */
  expanded: boolean;
  onToggle: () => void;
  now: Date;
}

/**
 * A P0 alarm above the tiers (mockup `.alarm`): the LLM's alarm reason and next action, independent of the score;
 * its points open the same score breakdown as a ranked row.
 */
export function AlarmRow({
  item,
  reviewHref,
  commentsHref,
  onWatch,
  watched,
  watching,
  onFilter,
  snapshotAt,
  onRejudge,
  rejudging,
  usageHref,
  onNavigate,
  current = false,
  focusRef,
  expanded,
  onToggle,
  now,
}: AlarmRowProps) {
  const points = Math.round(item.score.total_tenths / 10);
  const detailId = `detail-${item.key}`;
  return (
    <div
      ref={focusRef}
      role="group"
      tabIndex={-1}
      aria-label={`P0 alarm ${item.id}`}
      data-alarm={item.key}
      data-current={current || undefined}
      className={`grid grid-cols-[minmax(0,1fr)] sm:grid-cols-[auto_minmax(0,1fr)_auto] items-start gap-x-3.5 gap-y-1.5 rounded-lg border-2 border-high-bd bg-high-bg px-3.5 py-3 ${current ? "shadow-[inset_4px_0_0_var(--color-accent)]" : ""}`}
    >
      <Tooltip
        tip={`P0 alarm: judged by the LLM as needing your attention now. Independent of the score (${points}).`}
      >
        <span
          tabIndex={0}
          className="inline-block cursor-help rounded-md border border-high-bd bg-surface px-2 py-1 text-[13px] font-extrabold whitespace-nowrap text-high"
        >
          ▲ P0 alarm
        </span>
      </Tooltip>
      <div className="grid min-w-0 gap-1">
        <div className="text-[15px] font-bold [overflow-wrap:anywhere]">
          <WorkTitle item={item} />
        </div>
        <div className="flex flex-wrap items-center gap-1.5 text-xs text-muted">
          <span className="font-mono">{item.id}</span>
          <OpenExternal href={item.url} label={rowOpenLabel(item)} />
          <WorkFacts item={item} onFilter={onFilter} snapshotAt={snapshotAt} now={now} />
          <SizePill mark={item.change_size} onFilter={onFilter} />
          {item.kind === "pr" && (
            <CommentsInfo item={item} href={commentsHref} onNavigate={onNavigate} watched={watched} />
          )}
        </div>
        {item.llm ? (
          <>
            <LlmLine
              text={item.llm.alarm ?? item.llm.why}
              judgementDate={judgementDate(item)}
              marker={
                <WorkJudgement
                  item={item}
                  onRejudge={onRejudge}
                  rejudging={rejudging}
                  usageHref={usageHref}
                  onNavigate={onNavigate}
                />
              }
            />
            <LlmLine text={item.llm.next} label="Next" judgementDate={judgementDate(item)} />
          </>
        ) : (
          <WorkJudgement
            item={item}
            onRejudge={onRejudge}
            rejudging={rejudging}
            usageHref={usageHref}
            onNavigate={onNavigate}
          />
        )}
      </div>
      <div className="flex flex-wrap items-center justify-end gap-2">
        <ScoreButton item={item} expanded={expanded} onToggle={onToggle} controls={detailId} />
        <ItemAction item={item} reviewHref={reviewHref} commentsHref={commentsHref} onNavigate={onNavigate} />
        <RowMenu item={item} onWatch={onWatch} watched={watched} watching={watching} />
      </div>
      <div
        id={detailId}
        hidden={!expanded}
        className="col-span-full border-t border-dashed border-high-bd pt-1"
      >
        {expanded && <ScoreBreakdown item={item} now={now} />}
      </div>
    </div>
  );
}
