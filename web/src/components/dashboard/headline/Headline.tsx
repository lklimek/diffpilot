import { useEffect, useId, useRef, useState } from "react";
import { Tooltip } from "../../tooltip/Tooltip";
import { netTone, signed } from "../common/format";
import { Segmented } from "../common/Segmented";
import { BarPullRequests, type BarPullRequest } from "../bar-pull-requests/BarPullRequests";
import { NetChart, type NetPoint } from "../net-chart/NetChart";

/** Pull requests closed and opened in a period (OD16: pull requests only). */
export interface PeriodCount {
  closed: number;
  opened: number;
  /** Its name in the tile (`Mon–Wed`, `Tue 17 Nov`, `9–13 Nov`). */
  label: string;
}

/** Calendar periods in the dashboard time zone. */
export interface HeadlineFigures {
  today: PeriodCount;
  yesterday: PeriodCount;
  week: PeriodCount;
  month: PeriodCount;
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
  /** The bar of the shown chart whose pull requests are listed. */
  selected?: number | null;
  /** Lists a bar's pull requests (its index) or closes the list (null). */
  onSelect?: (index: number | null) => void;
  /** The pull requests of the selected bar. */
  listed?: { opened: BarPullRequest[]; closed: BarPullRequest[] } | null;
}

const net = (period: PeriodCount) => period.closed - period.opened;

function Tile({ label, value, sub, tip }: { label: string; value: number; sub: string; tip: string }) {
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

/** Today’s headline, three comparison periods and the longer chart. */
export function Headline({
  figures,
  days,
  weeks,
  unit,
  onUnitChange,
  staleNote,
  selected = null,
  onSelect,
  listed,
}: HeadlineProps) {
  const titleId = useId();
  const subId = useId();
  const chart = useRef<SVGSVGElement>(null);
  const headline = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState<number>();
  useEffect(() => {
    const element = headline.current;
    if (!element || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(([entry]) => {
      if (entry?.contentRect.height) setHeight(entry.contentRect.height + 8);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  const points = unit === "day" ? days : weeks;
  const bar = selected === null ? undefined : points[selected];
  const { today, yesterday, week, month } = figures;
  const minis = [
    ["Yesterday", yesterday],
    ["This week", week],
    ["This month", month],
  ] as const;
  return (
    <section
      aria-labelledby={titleId}
      className="@container grid gap-x-7 gap-y-1 rounded-lg border border-border bg-surface px-5 py-4"
    >
      <div className="grid items-start gap-x-7 gap-y-3 @min-[820px]:grid-cols-[minmax(250px,320px)_minmax(0,1fr)]">
        <div ref={headline} data-headline-figures className="min-w-0">
          <Tooltip tip="Today uses the time zone in Settings › GitHub sync. Net = closed − opened. Closed counts your PRs merged or closed without merge; opened counts PRs you opened. Positive means your open backlog shrank.">
            <h2 id={titleId} tabIndex={0} className="inline cursor-help text-[13px] font-semibold">
              Closed − opened, today
            </h2>
          </Tooltip>
          <div
            aria-describedby={subId}
            data-headline-value
            className={`mt-1.5 mb-0.5 text-[56px] leading-[1.05] font-bold tracking-tight tabular-nums ${netTone(net(today))}`}
          >
            {signed(net(today))}
          </div>
          <div id={subId} className="text-[13px] text-muted">
            {today.label}: closed <b className="text-text">{today.closed}</b> · opened{" "}
            <b className="text-text">{today.opened}</b> PRs
          </div>
          <div className="mt-3.5 grid grid-cols-3 gap-2">
            {minis.map(([label, period]) => (
              <Tile
                key={label}
                label={label}
                value={net(period)}
                sub={`${period.closed} − ${period.opened}`}
                tip={`${period.label}: closed ${period.closed}, opened ${period.opened}`}
              />
            ))}
          </div>
          {staleNote && <p className="mt-2 text-xs text-muted">{staleNote}</p>}
        </div>
        <div className="flex min-w-0 flex-col gap-1.5">
          <NetChart
            points={points}
            unit={unit}
            selected={selected}
            onSelect={onSelect}
            chartRef={chart}
            blockHeight={height}
            header={
              <div className="flex flex-wrap items-center gap-2.5">
                <h3 className="text-sm font-semibold">
                  Net per {unit === "day" ? "working day" : "week"}, with cumulative total
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
            }
          />
          {bar && listed && onSelect && (
            <BarPullRequests
              period={`${bar.full}${bar.partial ? " (so far)" : ""}`}
              opened={listed.opened}
              closed={listed.closed}
              onHide={() => onSelect(null)}
              focusAfter={() => chart.current}
            />
          )}
        </div>
      </div>
    </section>
  );
}
