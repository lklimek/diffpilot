import type { Ref } from "react";
import type { WorkItem } from "../../../api/generated/types";
import { Tooltip } from "../../tooltip/Tooltip";
import { LlmLine } from "../common/LlmLine";
import { OpenExternal, openLabel } from "../common/OpenExternal";
import type { Navigate } from "../common/navigate";
import { FactTags, SizePill } from "../common/Tags";
import { ScoreBreakdown } from "../score-breakdown/ScoreBreakdown";
import { ItemAction } from "../work-item/ItemAction";
import { itemPlace, provisionalPill, ScoreButton } from "../work-item/WorkItemRow";

interface AlarmRowProps {
  /** A P0 item. */
  item: WorkItem;
  /** In-app href of a pull request's review (plan §F.1). */
  reviewHref?: string;
  onNavigate?: Navigate;
  /** The keyboard cursor is on this alarm. */
  current?: boolean;
  /** The alarm's frame, focused by `j`/`k`. */
  focusRef?: Ref<HTMLDivElement>;
  /** The score breakdown is open. */
  expanded?: boolean;
  onToggle?: () => void;
  now: Date;
}

/**
 * A P0 alarm above the tiers (mockup `.alarm`): the LLM's alarm reason and next action, independent of the score;
 * its points open the same score breakdown as a ranked row.
 */
export function AlarmRow({
  item,
  reviewHref,
  onNavigate,
  current = false,
  focusRef,
  expanded = false,
  onToggle = () => {},
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
      className={`grid grid-cols-[auto_minmax(0,1fr)_auto] items-start gap-x-3.5 gap-y-1.5 rounded-lg border-2 border-high-bd bg-high-bg px-3.5 py-3 ${current ? "shadow-[inset_4px_0_0_var(--color-accent)]" : ""}`}
    >
      <Tooltip
        tip={`P0 alarm: judged by the LLM as needing your attention now. Independent of the score (${points}).`}
      >
        <span
          tabIndex={0}
          className="cursor-help rounded-md border border-high-bd bg-surface px-2 py-1 text-[13px] font-extrabold whitespace-nowrap text-high"
        >
          ▲ P0 alarm
        </span>
      </Tooltip>
      <div className="grid min-w-0 gap-1">
        <div className="text-[15px] font-bold [overflow-wrap:anywhere]">{item.title}</div>
        <div className="flex flex-wrap items-center gap-1.5 text-xs text-muted">
          <span className="font-mono">{item.id}</span>
          <OpenExternal href={item.url} label={openLabel(item)} />
          <span>
            {itemPlace(item)}
            {item.author ? ` · @${item.author}` : ""}
          </span>
          <SizePill mark={item.change_size} />
          <FactTags tags={item.tags} />
          {provisionalPill(item)}
        </div>
        {item.llm?.alarm && <LlmLine text={item.llm.alarm} />}
        {item.llm && <LlmLine text={item.llm.next} label="Next" />}
      </div>
      <div className="flex items-center justify-end gap-2">
        <ScoreButton item={item} expanded={expanded} onToggle={onToggle} controls={detailId} />
        <ItemAction item={item} reviewHref={reviewHref} onNavigate={onNavigate} />
      </div>
      <div id={detailId} hidden={!expanded} className="col-span-3 border-t border-dashed border-high-bd pt-1">
        {expanded && <ScoreBreakdown item={item} now={now} />}
      </div>
    </div>
  );
}
