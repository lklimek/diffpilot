import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
  type Ref,
} from "react";
import { Tooltip } from "../../tooltip/Tooltip";
import { netTone, signed } from "../common/format";

/** One bar of the chart: pull requests closed and opened in a working day or a week (bucketed by the screen). */
export interface NetPoint {
  /** Axis label (`26 Oct`). */
  label: string;
  /** Full name for the reading and the table (`Mon 26 Oct`, `Week of 26 Oct`). */
  full: string;
  /** The user's PRs merged or closed without merge. */
  closed: number;
  /** PRs the user opened. */
  opened: number;
  /** The current day or week, still running (drawn as a dashed outline). */
  partial: boolean;
  /** Label this bar on the axis. */
  tick: boolean;
}

/** A point with its net (closed − opened) and the cumulative total since the first point. */
export interface NetRow extends NetPoint {
  net: number;
  running: number;
}

/** Net and cumulative total of each point. */
export function netRows(points: NetPoint[]): NetRow[] {
  let running = 0;
  return points.map((point) => {
    const net = point.closed - point.opened;
    running += net;
    return { ...point, net, running };
  });
}

/** The point's name, `(so far)` while it is running. */
const describePeriod = (point: NetPoint) => `${point.full}${point.partial ? " (so far)" : ""}`;

const niceStep = (span: number) => [1, 2, 5, 10, 20, 50].find((step) => span / step <= 6) ?? 100;
const MARGIN = { l: 34, r: 46, t: 12, b: 26 };
const FALLBACK_WIDTH = 720;

/** Path of a bar with a rounded data end and a square base on the zero line. */
function barPath(x: number, width: number, zero: number, end: number): string {
  const h = Math.abs(end - zero);
  const r = Math.min(4, h, width / 2);
  const right = x + width;
  return end < zero
    ? `M${x},${zero}V${end + r}Q${x},${end} ${x + r},${end}H${right - r}Q${right},${end} ${right},${end + r}V${zero}Z`
    : `M${x},${zero}V${end - r}Q${x},${end} ${x + r},${end}H${right - r}Q${right},${end} ${right},${end - r}V${zero}Z`;
}

interface NetChartProps {
  /** Oldest first; the last one may be partial. */
  points: NetPoint[];
  /** What one point covers. */
  unit: "day" | "week";
  /** The bar whose pull requests are listed. */
  selected?: number | null;
  /** Lists a bar's pull requests (its index), or closes the list (null); without it bars are not chosen. */
  onSelect?: (index: number | null) => void;
  /** The chart's focusable SVG. */
  chartRef?: Ref<SVGSVGElement>;
  header: ReactNode;
  blockHeight?: number;
}

/**
 * Net closed − opened per day or week as bars, with the cumulative total as a line (mockup `drawNet`). Hand-written
 * SVG; focusable, ←/→/Home/End read one point into a live region; the same numbers as a table on request. With
 * `onSelect`, a click or Enter/Space on a bar chooses it; choosing it again lets it go.
 */
export function NetChart({
  points,
  unit,
  selected = null,
  onSelect,
  chartRef,
  header,
  blockHeight,
}: NetChartProps) {
  const box = useRef<HTMLDivElement>(null);
  const [[width, height], setSize] = useState([FALLBACK_WIDTH, 128]);
  const [at, setAt] = useState<number | null>(null);
  const [reading, setReading] = useState("");
  const [table, setTable] = useState(false);

  useEffect(() => {
    const element = box.current;
    if (!element || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(([entry]) => {
      if (entry && entry.contentRect.width > 0) {
        setSize([entry.contentRect.width, Math.max(40, entry.contentRect.height)]);
      }
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const rows = netRows(points);
  // Opening and closing the list is announced, whatever closed it (the bar again, Hide, the period switch).
  const chosen = selected !== null && rows[selected] ? describePeriod(rows[selected]) : null;
  const [announced, setAnnounced] = useState(chosen);
  if (announced !== chosen) {
    setAnnounced(chosen);
    if (chosen !== null) setReading(`Pull requests of ${chosen} listed under the chart`);
    else if (announced !== null) setReading(`Pull requests of ${announced} hidden`);
  }
  if (rows.length === 0) {
    return (
      <>
        {header}
        <p className="text-xs text-muted">No pull request activity in this period.</p>
      </>
    );
  }
  const last = rows.at(-1)!;
  const iw = width - MARGIN.l - MARGIN.r;
  const ih = height - MARGIN.t - MARGIN.b;
  const values = rows.flatMap((row) => [row.net, row.running]);
  const step = niceStep(Math.max(1, ...values) - Math.min(0, ...values));
  const lo = Math.floor(Math.min(0, ...values) / step) * step;
  const hi = Math.ceil(Math.max(1, ...values) / step) * step;
  const y = (v: number) => MARGIN.t + ih - ((v - lo) / (hi - lo)) * ih;
  const slot = iw / rows.length;
  const bar = Math.min(26, slot * 0.6);
  const cx = (i: number) => MARGIN.l + slot * i + slot / 2;
  const grid: number[] = [];
  for (let v = lo; v <= hi; v += step) grid.push(v);
  const now = unit === "day" ? "today" : "this week";

  const describe = (row: NetRow) =>
    `${describePeriod(row)}: closed ${row.closed}, opened ${row.opened}, net ${signed(row.net)}, cumulative ${signed(row.running)}`;
  const show = (i: number, read: boolean) => {
    setAt(i);
    if (read) setReading(describe(rows[i]!));
  };
  const choose = (i: number) => {
    onSelect?.(i === selected ? null : i);
  };
  const indexAt = (event: MouseEvent<SVGSVGElement>) => {
    const i = Math.floor(
      (event.clientX - event.currentTarget.getBoundingClientRect().left - MARGIN.l) / slot,
    );
    return i >= 0 && i < rows.length ? i : null;
  };
  const onKeyDown = (event: KeyboardEvent<SVGSVGElement>) => {
    const current = at ?? selected ?? rows.length - 1;
    if (onSelect && (event.key === "Enter" || event.key === " ")) {
      event.preventDefault();
      choose(current);
      return;
    }
    const next = { ArrowLeft: current - 1, ArrowRight: current + 1, Home: 0, End: rows.length - 1 }[
      event.key
    ];
    if (next === undefined) return;
    event.preventDefault();
    show(Math.max(0, Math.min(rows.length - 1, next)), true);
  };
  const hover = at === null ? null : rows[at]!;
  const tipLeft = at === null ? 0 : Math.max(0, Math.min(cx(at) + 14, width - 220));

  return (
    <div className="flex min-w-0 flex-col gap-1.5">
      <div
        data-headline-chart
        className="flex min-w-0 flex-col gap-1.5"
        style={{ height: blockHeight ?? 178 }}
      >
        {header}
        <div aria-hidden className="flex flex-wrap gap-x-3 gap-y-0.5 text-[11px] text-muted">
          <span>
            <i className="mr-1.5 inline-block h-3 w-3 rounded-sm bg-c-pos align-[-2px]" />
            closed more than opened
          </span>
          <span>
            <i className="mr-1.5 inline-block h-3 w-3 rounded-sm bg-c-neg align-[-2px]" />
            opened more than closed
          </span>
          <span>
            <i className="mr-1.5 inline-block h-0.5 w-4 rounded-sm bg-text align-[3px]" />
            cumulative total since the first {unit} shown
          </span>
          <span>
            <i className="mr-1.5 inline-block h-3 w-3 rounded-sm border-[1.5px] border-dashed border-c-pos align-[-2px]" />
            {now} so far
          </span>
        </div>
        <div ref={box} className="relative min-h-0 w-full flex-1">
          <svg
            width={width}
            height={height}
            viewBox={`0 0 ${width} ${height}`}
            role="img"
            tabIndex={0}
            aria-label={`Net closed minus opened pull requests per ${unit} for the last ${rows.length} ${unit}s; cumulative total ${signed(last.running)}. Use the left and right arrow keys to read each ${unit}.${onSelect ? ` Enter or Space lists the pull requests of the ${unit} read.` : ""}`}
            ref={chartRef}
            onKeyDown={onKeyDown}
            onFocus={() => show(at ?? selected ?? rows.length - 1, true)}
            onBlur={() => setAt(null)}
            onMouseMove={(event) => {
              const i = indexAt(event);
              if (i !== null) show(i, false);
              else setAt(null);
            }}
            onMouseLeave={() => setAt(null)}
            onClick={(event) => {
              const i = indexAt(event);
              if (i === null) return;
              setAt(i);
              choose(i);
            }}
            className={`absolute inset-0 block w-full overflow-visible rounded focus-visible:outline-offset-4 ${onSelect ? "cursor-pointer" : ""}`}
          >
            {/* Under the bars: the bar whose pull requests are listed. */}
            {selected !== null && selected < rows.length && (
              <rect
                x={MARGIN.l + slot * selected}
                y={MARGIN.t}
                width={slot}
                height={ih}
                className="fill-accent opacity-[.14]"
              />
            )}
            {grid.map((v) => (
              <g key={v}>
                <line
                  x1={MARGIN.l}
                  x2={width - MARGIN.r}
                  y1={y(v)}
                  y2={y(v)}
                  className={v === 0 ? "stroke-muted" : "stroke-border opacity-60"}
                />
                <text
                  x={MARGIN.l - 6}
                  y={y(v) + 4}
                  textAnchor="end"
                  className={`text-[11px] ${v === 0 ? "fill-text font-semibold" : "fill-muted"}`}
                >
                  {signed(v)}
                </text>
              </g>
            ))}
            {rows.map((row, i) => (
              <g key={`${row.label}-${i}`} data-point={i} data-selected={i === selected || undefined}>
                {/* The running day or week: a dashed outline, visible on both themes and at net 0. */}
                {row.partial ? (
                  <path
                    data-partial
                    d={
                      row.net !== 0
                        ? barPath(cx(i) - bar / 2, bar, y(0), y(row.net))
                        : `M${cx(i) - bar / 2},${y(0) - 3}h${bar}v6h${-bar}Z`
                    }
                    strokeWidth={1.5}
                    strokeDasharray="4 3"
                    className={`fill-none ${row.net < 0 ? "stroke-c-neg" : "stroke-c-pos"}`}
                  />
                ) : row.net !== 0 ? (
                  <path
                    d={barPath(cx(i) - bar / 2, bar, y(0), y(row.net))}
                    className={row.net > 0 ? "fill-c-pos" : "fill-c-neg"}
                  />
                ) : (
                  <line
                    x1={cx(i) - bar / 2}
                    x2={cx(i) + bar / 2}
                    y1={y(0)}
                    y2={y(0)}
                    strokeWidth={3}
                    className="stroke-muted"
                  />
                )}
                {row.tick && (width >= 420 || i % 2 === 0 || row.partial) && (
                  <text x={cx(i)} y={height - 6} textAnchor="middle" className="fill-muted text-[11px]">
                    {row.partial ? (unit === "day" ? "today" : "this wk") : row.label}
                  </text>
                )}
              </g>
            ))}
            <path
              d={rows.map((row, i) => `${i ? "L" : "M"}${cx(i)},${y(row.running)}`).join("")}
              fill="none"
              strokeWidth={2}
              strokeLinejoin="round"
              className="stroke-text"
            />
            <circle
              r={4.5}
              cx={cx(rows.length - 1)}
              cy={y(last.running)}
              strokeWidth={2}
              className="fill-text stroke-surface"
            />
            <text x={cx(rows.length - 1) + 9} y={y(last.running) + 4} className="fill-text text-xs font-bold">
              {signed(last.running)}
            </text>
            {at !== null && (
              <rect
                x={MARGIN.l + slot * at}
                y={MARGIN.t}
                width={slot}
                height={ih}
                className="fill-text opacity-[.06]"
              />
            )}
          </svg>
          {hover && (
            <div
              aria-hidden
              data-chart-tip
              style={{ left: tipLeft, top: MARGIN.t + 4 }}
              className="pointer-events-none absolute z-[5] w-max max-w-[220px] rounded-md bg-text px-2.5 py-1.5 text-xs leading-snug text-bg shadow-tip"
            >
              <b>
                {hover.full}
                {hover.partial ? " (so far)" : ""}
              </b>
              <br />
              closed {hover.closed} · opened {hover.opened}
              <br />
              {/* Neutral: the inverted tooltip gives the net tokens under 2.3:1 in both themes. */}
              net <b>{signed(hover.net)}</b> · cumulative <b>{signed(hover.running)}</b>
              {onSelect && (
                <>
                  <br />
                  {at === selected
                    ? "Click or press Enter to hide its pull requests"
                    : "Click or press Enter to list its pull requests"}
                </>
              )}
            </div>
          )}
          <span className="sr-only" aria-live="polite" data-chart-reading>
            {reading}
          </span>
        </div>
      </div>
      <div>
        <Tooltip tip="Show the same numbers as a table">
          <button
            type="button"
            aria-expanded={table}
            onClick={() => setTable((open) => !open)}
            className="min-h-6 cursor-pointer border-0 bg-transparent p-0 text-xs text-accent underline"
          >
            {table ? "Hide table" : "Show as table"}
          </button>
        </Tooltip>
      </div>
      {table && (
        <div
          role="region"
          tabIndex={0}
          aria-label={`Net per ${unit} as a table`}
          className="max-h-[260px] overflow-auto rounded-md border border-border"
        >
          <table className="w-full max-w-[640px] border-collapse text-[12.5px] tabular-nums [&_td]:border-b [&_td]:border-border [&_td]:px-2.5 [&_td]:py-0.5 [&_td]:text-right [&_td:first-child]:text-left [&_th]:border-b [&_th]:border-border [&_th]:px-2.5 [&_th]:py-0.5 [&_th]:text-right [&_th:first-child]:text-left">
            <caption className="sr-only">Net closed minus opened pull requests</caption>
            <thead>
              <tr>
                <th scope="col">{unit === "day" ? "Day" : "Week"}</th>
                <th scope="col">Closed</th>
                <th scope="col">Opened</th>
                <th scope="col">Net</th>
                <th scope="col">Cumulative</th>
              </tr>
            </thead>
            <tbody>
              {[...rows].reverse().map((row, i) => (
                <tr key={`${row.full}-${i}`}>
                  <td>
                    {row.full}
                    {row.partial ? " (so far)" : ""}
                  </td>
                  <td>{row.closed}</td>
                  <td>{row.opened}</td>
                  <td className={netTone(row.net)}>{signed(row.net)}</td>
                  <td className={netTone(row.running)}>{signed(row.running)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
