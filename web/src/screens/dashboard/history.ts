// History bucketing of the dashboard charts (plan §C.1, §F.3): the snapshot's dated pull-request and review events
// counted by the browser's local working day and week. Pull requests only (OD16): closed = merged or closed
// unmerged, opened = opened; reviews given are counted apart and never enter the net.
import type { HistoryEvent, HistoryKind } from "../../api/generated/types";
import type { BarPullRequest } from "../../components/dashboard/bar-pull-requests/BarPullRequests";

/** One bucket of the net chart; `day` is its first local date (`YYYY-MM-DD`; Monday for weeks). */
export interface NetBucket {
  day: string;
  closed: number;
  opened: number;
  /** closed − opened. */
  net: number;
  /** Sum of `net` from the first bucket shown up to this one. */
  running: number;
  /** The bucket holding now: still filling. */
  partial: boolean;
}

/** Events of one kind set in one week. */
export interface WeekCount {
  day: string;
  count: number;
  partial: boolean;
}

/** Closed and opened pull requests of a period. */
export interface NetCount {
  closed: number;
  opened: number;
  net: number;
}

/** A period of the headline with its name in the tile (`Mon–Wed`, `Tue 17 Nov`, `9–13 Nov`). */
export type Period = NetCount & { label: string };

/** The "Yesterday" side section: pull requests and reviews of the previous working day. */
export interface YesterdaySummary {
  day: string;
  prs: NetCount & { merged: number };
  reviews: number;
  /** `owner/name#number` of the pull requests merged that day, in event order. */
  mergedIds: string[];
}

/** Working days of the day chart and of each half of the four-weeks tile. */
export const CHART_DAYS = 20;
/** Weeks of the week chart (the snapshot's history window, plan §C.1). */
export const CHART_WEEKS = 12;
/** Weeks of the throughput mini charts. */
export const THROUGHPUT_WEEKS = 8;

const DAY = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short" });
const WEEKDAY_DAY = new Intl.DateTimeFormat("en-GB", { weekday: "short", day: "numeric", month: "short" });

/** Local midnight of a `YYYY-MM-DD` date. */
function parseDay(day: string): Date {
  const [y, m, d] = day.split("-").map(Number);
  return new Date(y!, m! - 1, d!);
}

/** `9–13 Nov`, or `30 Nov – 4 Dec` across months. */
function rangeLabel(from: Date, to: Date): string {
  return from.getMonth() === to.getMonth()
    ? `${from.getDate()}–${DAY.format(to)}`
    : `${DAY.format(from)} – ${DAY.format(to)}`;
}

const CLOSED: readonly HistoryKind[] = ["pr_merged", "pr_closed_unmerged"];
const OPENED: readonly HistoryKind[] = ["pr_opened"];

/** Local date of `d` as `YYYY-MM-DD`. */
export const localDay = (d: Date): string =>
  `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

const midnight = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate());
const addDays = (d: Date, days: number) => new Date(d.getFullYear(), d.getMonth(), d.getDate() + days);

/** Local midnight of the working day `d` counts on: Saturday and Sunday count on the Friday before. */
export function workingDay(d: Date): Date {
  const day = d.getDay();
  return addDays(midnight(d), day === 6 ? -1 : day === 0 ? -2 : 0);
}

/** Monday (local midnight) of the week of `d`. */
const monday = (d: Date) => addDays(midnight(d), -((d.getDay() + 6) % 7));

/** The working day before working day `d`. */
const previousWorkingDay = (d: Date) => workingDay(addDays(d, -1));

/** `count` working days ending with the one holding `now`, oldest first. */
function workingDays(now: Date, count: number): Date[] {
  const days = [workingDay(now)];
  while (days.length < count) days.unshift(previousWorkingDay(days[0]!));
  return days;
}

/** Counts of `kinds` per working day (`YYYY-MM-DD`). */
function perWorkingDay(events: readonly HistoryEvent[], kinds: readonly HistoryKind[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const e of events) {
    if (!kinds.includes(e.kind)) continue;
    const day = localDay(workingDay(new Date(e.at)));
    counts.set(day, (counts.get(day) ?? 0) + 1);
  }
  return counts;
}

/** Counts of `kinds` from `from` (inclusive) to `to` (exclusive), local midnights. */
function countBetween(events: readonly HistoryEvent[], kinds: readonly HistoryKind[], from: Date, to: Date) {
  return events.filter((e) => {
    const t = workingDay(new Date(e.at)).getTime();
    return kinds.includes(e.kind) && t >= from.getTime() && t < to.getTime();
  }).length;
}

const netBetween = (events: readonly HistoryEvent[], from: Date, to: Date): NetCount => {
  const closed = countBetween(events, CLOSED, from, to);
  const opened = countBetween(events, OPENED, from, to);
  return { closed, opened, net: closed - opened };
};

/** Adds `running` to buckets in order. */
function withRunning(buckets: Omit<NetBucket, "running">[]): NetBucket[] {
  let running = 0;
  return buckets.map((b) => ({ ...b, running: (running += b.net) }));
}

/** The last `count` working days with closed, opened, net and running total; the one holding now is partial. */
export function dailySeries(events: readonly HistoryEvent[], now: Date, count = CHART_DAYS): NetBucket[] {
  const closed = perWorkingDay(events, CLOSED);
  const opened = perWorkingDay(events, OPENED);
  const today = localDay(workingDay(now));
  return withRunning(
    workingDays(now, count).map((d) => {
      const day = localDay(d);
      const c = closed.get(day) ?? 0;
      const o = opened.get(day) ?? 0;
      return { day, closed: c, opened: o, net: c - o, partial: day === today };
    }),
  );
}

/** The last `count` weeks (Monday to Sunday) with closed, opened, net and running total; this week is partial. */
export function weeklySeries(events: readonly HistoryEvent[], now: Date, count = CHART_WEEKS): NetBucket[] {
  const thisWeek = monday(now);
  return withRunning(
    Array.from({ length: count }, (_, i) => {
      const from = addDays(thisWeek, 7 * (i - count + 1));
      return { day: localDay(from), ...netBetween(events, from, addDays(from, 7)), partial: i === count - 1 };
    }),
  );
}

/** Events of `kinds` per week for the last `count` weeks; this week is partial. */
export function weeklyCounts(
  events: readonly HistoryEvent[],
  kinds: readonly HistoryKind[],
  now: Date,
  count = THROUGHPUT_WEEKS,
): WeekCount[] {
  const thisWeek = monday(now);
  return Array.from({ length: count }, (_, i) => {
    const from = addDays(thisWeek, 7 * (i - count + 1));
    return {
      day: localDay(from),
      count: countBetween(events, kinds, from, addDays(from, 7)),
      partial: i === count - 1,
    };
  });
}

/** Calendar-day headline figures in the configured dashboard zone; chart buckets keep their working-day rules. */
export function headline(events: readonly HistoryEvent[], now: Date, timeZone?: string | null) {
  const calendar = new Intl.DateTimeFormat("en-GB", {
    timeZone: timeZone ?? undefined,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  });
  const inZone = (date: Date) => {
    const parts = calendar.formatToParts(date);
    const part = (name: Intl.DateTimeFormatPartTypes) =>
      Number(parts.find(({ type }) => type === name)?.value);
    return new Date(part("year"), part("month") - 1, part("day"));
  };
  const today = inZone(now);
  const yesterday = addDays(today, -1);
  const weekStart = monday(today);
  const monthStart = new Date(today.getFullYear(), today.getMonth(), 1);
  const dated = events.flatMap((event) => {
    const at = Date.parse(event.at);
    return Number.isFinite(at) && at <= now.getTime() ? [{ at: inZone(new Date(at)), kind: event.kind }] : [];
  });
  const period = (from: Date, to: Date, label: string): Period => {
    const kinds = dated.filter(({ at }) => at >= from && at <= to).map(({ kind }) => kind);
    const closed = kinds.filter((kind) => CLOSED.includes(kind)).length;
    const opened = kinds.filter((kind) => OPENED.includes(kind)).length;
    return { closed, opened, net: closed - opened, label };
  };
  return {
    today: period(today, today, WEEKDAY_DAY.format(today)),
    yesterday: period(yesterday, yesterday, WEEKDAY_DAY.format(yesterday)),
    week: period(weekStart, today, rangeLabel(weekStart, today)),
    month: period(monthStart, today, rangeLabel(monthStart, today)),
  };
}

/** Pull requests and reviews of the previous working day. */
export function yesterday(events: readonly HistoryEvent[], now: Date): YesterdaySummary {
  const from = previousWorkingDay(workingDay(now));
  const to = addDays(from, 1);
  const inDay = (e: HistoryEvent) => {
    const t = workingDay(new Date(e.at)).getTime();
    return t >= from.getTime() && t < to.getTime();
  };
  const merged = events.filter((e) => e.kind === "pr_merged" && inDay(e));
  return {
    day: localDay(from),
    prs: { ...netBetween(events, from, to), merged: merged.length },
    reviews: countBetween(events, ["review_given"], from, to),
    mergedIds: merged.map((e) => e.item_id),
  };
}

/** The day of `yesterday()` as the section names it (`Tue 17 Nov`). */
export const fullDayLabel = (day: string): string => WEEKDAY_DAY.format(parseDay(day));

/** A bucket of the net chart with its axis label, full name and whether the axis labels it. */
export type ChartPoint = NetBucket & { label: string; full: string; tick: boolean };

/** Labels the buckets of the day or week chart: days by date with Mondays on the axis, weeks every second. */
export function chartPoints(buckets: readonly NetBucket[], unit: "day" | "week"): ChartPoint[] {
  return buckets.map((b, i) => {
    const start = parseDay(b.day);
    const last = i === buckets.length - 1;
    return {
      ...b,
      label: DAY.format(start),
      full: unit === "day" ? WEEKDAY_DAY.format(start) : `Week of ${DAY.format(start)}`,
      tick: last || (unit === "day" ? start.getDay() === 1 : i % 2 === 1),
    };
  });
}

/** One week of the throughput charts: the user's PRs merged and the reviews given. */
export interface ThroughputPoint {
  label: string;
  merged: number;
  reviews: number;
  partial: boolean;
}

/** Merged pull requests and reviews given per week, the last `count` weeks; this week is partial. */
export function throughputWeeks(
  events: readonly HistoryEvent[],
  now: Date,
  count = THROUGHPUT_WEEKS,
): ThroughputPoint[] {
  const merged = weeklyCounts(events, ["pr_merged"], now, count);
  const reviews = weeklyCounts(events, ["review_given"], now, count);
  return merged.map((week, i) => ({
    label: DAY.format(parseDay(week.day)),
    merged: week.count,
    reviews: reviews[i]!.count,
    partial: week.partial,
  }));
}

// GitHub logins: letters, digits and hyphens; repository names add `_` and `.` (but are never `.` or `..`).
const PR_ID = /^([A-Za-z0-9-]+)\/([A-Za-z0-9_.-]+)#(\d+)$/;

/** GitHub link of a pull request `owner/name#number`; null for anything else. */
export function pullRequestUrl(id: string): string | null {
  const match = PR_ID.exec(id);
  if (!match || match[2] === "." || match[2] === "..") return null;
  return `https://github.com/${match[1]}/${match[2]}/pull/${match[3]}`;
}

/**
 * The pull requests behind one bar of the net chart (the bar starting at `day`), bucketed as the chart buckets
 * them; `titles` maps `owner/name#number` to the titles the snapshot knows. In event order.
 */
export function barPullRequests(
  events: readonly HistoryEvent[],
  day: string,
  unit: "day" | "week",
  titles: ReadonlyMap<string, string>,
): { opened: BarPullRequest[]; closed: BarPullRequest[] } {
  const inBar = events
    .filter((e) => {
      const bucket = workingDay(new Date(e.at));
      return localDay(unit === "day" ? bucket : monday(bucket)) === day;
    })
    .sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
  const pr = (e: HistoryEvent): BarPullRequest => ({
    id: e.item_id,
    title: titles.get(e.item_id) ?? null,
    url: pullRequestUrl(e.item_id),
  });
  return {
    opened: inBar.filter((e) => OPENED.includes(e.kind)).map(pr),
    closed: inBar
      .filter((e) => CLOSED.includes(e.kind))
      .map((e) => ({ ...pr(e), outcome: e.kind === "pr_merged" ? "merged" : "closed_unmerged" })),
  };
}
