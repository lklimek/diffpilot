import { type ReactNode, useId } from "react";
import { Tooltip } from "../../tooltip/Tooltip";
import { netTone, signed } from "../common/format";
import { Segmented } from "../common/Segmented";
import { NetChart, type NetPoint } from "../net-chart/NetChart";

/** Pull requests closed and opened in a period (OD16: pull requests only). */
export interface PeriodCount {
  closed: number;
  opened: number;
  /** Its name in the tile (`Mon–Wed`, `Tue 17 Nov`, `9–13 Nov`). */
  label: string;
}

/** Numbers of the headline, bucketed by the screen in the browser's time zone (plan §C.1). */
export interface HeadlineFigures {
  /** This week so far. */
  week: PeriodCount;
  /** The previous working day. */
  yesterday: PeriodCount;
  /** The previous week. */
  lastWeek: PeriodCount;
  /** Net of the last 20 working days, and of the 20 before them. */
  fourWeeks: { net: number; before: number };
}

/** Chart granularity. */
export type ChartUnit = "day" | "week";

interface HeadlineProps {
  figures: HeadlineFigures;
  /** Last 20 working days, oldest first. */
  days: NetPoint[];
  /** Last 12 weeks, oldest first. */
  weeks: NetPoint[];
  unit: ChartUnit;
  onUnitChange: (unit: ChartUnit) => void;
  /** Shown under the tiles while the numbers come from an older snapshot (`From the 08:12 snapshot.`). */
  staleNote?: string | null;
}

const net = (period: PeriodCount) => period.closed - period.opened;

function Tile({ label, value, sub, tip }: { label: string; value: number; sub: ReactNode; tip: string }) {
  return (
    <Tooltip tip={tip}>
      <div tabIndex={0} className="cursor-help rounded-md border border-border bg-surface-2 px-2.5 py-1.5">
        <div className="text-[11px] text-muted">{label}</div>
        <div className="font-mono text-lg font-bold tabular-nums">
          <span className={netTone(value)}>{signed(value)}</span>
        </div>
        <div className="text-[11px] text-muted">{sub}</div>
      </div>
    </Tooltip>
  );
}

/**
 * The main metric "PRs closed minus PRs opened" (mockup `renderHeadline`): this week as the hero, tiles for
 * yesterday, last week and four weeks, and the net chart by days or weeks. Pull requests only; no scope switch.
 */
export function Headline({ figures, days, weeks, unit, onUnitChange, staleNote }: HeadlineProps) {
  const titleId = useId();
  const subId = useId();
  const { week, yesterday, lastWeek, fourWeeks } = figures;
  return (
    <section
      aria-labelledby={titleId}
      className="@container grid gap-x-7 gap-y-1 rounded-lg border border-border bg-surface px-5 py-4"
    >
      <div className="grid gap-x-7 gap-y-3 @min-[820px]:grid-cols-[minmax(250px,320px)_minmax(0,1fr)]">
        <div>
          <Tooltip tip="Net = closed − opened. Closed counts your PRs merged or closed without merge; opened counts PRs you opened. Positive means your open backlog shrank.">
            <h2 id={titleId} tabIndex={0} className="inline cursor-help text-[13px] font-semibold">
              Closed − opened, this week
            </h2>
          </Tooltip>
          <div
            aria-describedby={subId}
            data-headline-value
            className={`mt-1.5 mb-0.5 text-[56px] leading-[1.05] font-bold tracking-tight tabular-nums ${netTone(net(week))}`}
          >
            {signed(net(week))}
          </div>
          <div id={subId} className="text-[13px] text-muted">
            {week.label}: closed <b className="text-text">{week.closed}</b> · opened{" "}
            <b className="text-text">{week.opened}</b> PRs
          </div>
          <div className="mt-3.5 grid grid-cols-3 gap-2">
            <Tile
              label="Yesterday"
              value={net(yesterday)}
              sub={`${yesterday.closed} − ${yesterday.opened}`}
              tip={`${yesterday.label}: closed ${yesterday.closed}, opened ${yesterday.opened}`}
            />
            <Tile
              label="Last week"
              value={net(lastWeek)}
              sub={lastWeek.label}
              tip={`${lastWeek.label}: closed ${lastWeek.closed}, opened ${lastWeek.opened}`}
            />
            <Tile
              label="4 weeks"
              value={fourWeeks.net}
              sub={
                <>
                  before: <span className={netTone(fourWeeks.before)}>{signed(fourWeeks.before)}</span>
                </>
              }
              tip={`Net of the last 20 working days; the 20 before them: ${signed(fourWeeks.before)}`}
            />
          </div>
          {staleNote && <p className="mt-2 text-xs text-muted">{staleNote}</p>}
        </div>
        <div className="flex min-w-0 flex-col gap-1.5">
          <div className="flex flex-wrap items-center gap-2.5">
            <h3 className="text-sm font-semibold">
              Net per {unit === "day" ? "working day" : "week"}, with running total
            </h3>
            <span className="flex-1" />
            <Segmented
              label="Period"
              value={unit}
              onChange={onUnitChange}
              options={[
                { value: "day", label: "Days", tip: "Last 20 working days" },
                { value: "week", label: "Weeks", tip: "Last 12 weeks" },
              ]}
            />
          </div>
          <NetChart points={unit === "day" ? days : weeks} unit={unit} />
        </div>
      </div>
    </section>
  );
}
