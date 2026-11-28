// History bucketing of the dashboard charts (plan §C.1, §F.3): the snapshot's dated pull-request and review events
// counted by the dashboard's calendar day and week. Pull requests only (OD16): closed = merged or closed
// unmerged, opened = opened; reviews given are counted apart and never enter the net.
import type { HistoryEvent, HistoryKind, PrRef } from "../../api/generated/types";
import type { BarPullRequest } from "../../components/dashboard/bar-pull-requests/BarPullRequests";
import { parsePrRef, prPath } from "../pr/paths";

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

/** The "Yesterday" side section: pull requests and reviews of the previous calendar day. */
export interface YesterdaySummary {
  day: string;
  prs: NetCount & { merged: number };
  reviews: number;
  /** `owner/name#number` of the pull requests merged that day, in event order. */
  mergedIds: string[];
}

/** Calendar days of the day chart. */
export const CHART_DAYS = 20;
/** Weeks of the week chart (the snapshot's history window, plan §C.1). */
export const CHART_WEEKS = 12;
/** Weeks of the throughput mini charts. */
export const THROUGHPUT_WEEKS = 8;

const DAY = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short", timeZone: "UTC" });
const WEEKDAY_DAY = new Intl.DateTimeFormat("en-GB", {
  weekday: "short",
  day: "numeric",
  month: "short",
  timeZone: "UTC",
});

const formatDay = (formatter: Intl.DateTimeFormat, d: Date) =>
  formatter.format(Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()));

/** Local midnight of a `YYYY-MM-DD` date. */
function parseDay(day: string): Date {
  const [y, m, d] = day.split("-").map(Number);
  return new Date(y!, m! - 1, d!);
}

/** `9–13 Nov`, or `30 Nov – 4 Dec` across months. */
function rangeLabel(from: Date, to: Date): string {
  if (from.getTime() === to.getTime()) return formatDay(WEEKDAY_DAY, to);
  return from.getMonth() === to.getMonth()
    ? `${from.getDate()}–${formatDay(DAY, to)}`
    : `${formatDay(DAY, from)} – ${formatDay(DAY, to)}`;
}

const CLOSED: readonly HistoryKind[] = ["pr_merged", "pr_closed_unmerged"];
const OPENED: readonly HistoryKind[] = ["pr_opened"];

type ChartEvent = HistoryEvent & { calendarDay?: string };

const eventDate = (event: ChartEvent) =>
  event.calendarDay === undefined ? new Date(event.at) : parseDay(event.calendarDay);

/** Local date of `d` as `YYYY-MM-DD`. */
export const localDay = (d: Date): string =>
  `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

const midnight = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate());
const addDays = (d: Date, days: number) => new Date(d.getFullYear(), d.getMonth(), d.getDate() + days);

/** Monday (local midnight) of the week of `d`. */
const monday = (d: Date) => addDays(midnight(d), -((d.getDay() + 6) % 7));

/** Counts of `kinds` per calendar day (`YYYY-MM-DD`). */
function perDay(events: readonly ChartEvent[], kinds: readonly HistoryKind[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const e of events) {
    if (!kinds.includes(e.kind)) continue;
    const day = e.calendarDay ?? localDay(new Date(e.at));
    counts.set(day, (counts.get(day) ?? 0) + 1);
  }
  return counts;
}

/** Counts of `kinds` from `from` (inclusive) to `to` (exclusive), local midnights. */
function countBetween(events: readonly ChartEvent[], kinds: readonly HistoryKind[], from: Date, to: Date) {
  return events.filter((e) => {
    const t = eventDate(e).getTime();
    return kinds.includes(e.kind) && t >= from.getTime() && t < to.getTime();
  }).length;
}

const netBetween = (events: readonly ChartEvent[], from: Date, to: Date): NetCount => {
  const closed = countBetween(events, CLOSED, from, to);
  const opened = countBetween(events, OPENED, from, to);
  return { closed, opened, net: closed - opened };
};

/** Adds `running` to buckets in order. */
function withRunning(buckets: Omit<NetBucket, "running">[]): NetBucket[] {
  let running = 0;
  return buckets.map((b) => ({ ...b, running: (running += b.net) }));
}

/** The last `count` calendar days with closed, opened, net and running total; today is partial. */
export function dailySeries(events: readonly ChartEvent[], now: Date, count = CHART_DAYS): NetBucket[] {
  const closed = perDay(events, CLOSED);
  const opened = perDay(events, OPENED);
  return withRunning(
    Array.from({ length: count }, (_, i) => {
      const day = localDay(addDays(now, i - count + 1));
      const c = closed.get(day) ?? 0;
      const o = opened.get(day) ?? 0;
      return { day, closed: c, opened: o, net: c - o, partial: i === count - 1 };
    }),
  );
}

/** The last `count` weeks (Monday to Sunday) with closed, opened, net and running total; this week is partial. */
export function weeklySeries(events: readonly ChartEvent[], now: Date, count = CHART_WEEKS): NetBucket[] {
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
  events: readonly ChartEvent[],
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

function dayFormatter(timeZone?: string | null) {
  const options = { year: "numeric", month: "2-digit", day: "2-digit" } as const;
  try {
    return {
      calendar: new Intl.DateTimeFormat("en-GB", { ...options, timeZone: timeZone ?? undefined }),
      unsupportedZone: null,
    };
  } catch (error) {
    if (!(error instanceof RangeError)) throw error;
    return { calendar: new Intl.DateTimeFormat("en-GB", options), unsupportedZone: timeZone ?? null };
  }
}

/** Calendar-day figures for the headline and side panel. */
export function headline(events: readonly HistoryEvent[], now: Date, timeZone?: string | null) {
  const { day, unsupportedZone, zoneLabel } = dashboardCalendar(timeZone);
  const today = day(now);
  const yesterday = addDays(today, -1);
  const weekStart = monday(today);
  const monthStart = new Date(today.getFullYear(), today.getMonth(), 1);
  const dated = events.flatMap((event) => {
    const at = Date.parse(event.at);
    return Number.isFinite(at) && at <= now.getTime() ? [{ ...event, day: day(new Date(at)).getTime() }] : [];
  });
  const inPeriod = (from: Date, to: Date) =>
    dated.filter(({ day }) => day >= from.getTime() && day <= to.getTime());
  const net = (items: readonly HistoryEvent[]): NetCount => {
    const closed = items.filter(({ kind }) => CLOSED.includes(kind)).length;
    const opened = items.filter(({ kind }) => OPENED.includes(kind)).length;
    return { closed, opened, net: closed - opened };
  };
  const period = (from: Date, to: Date): Period => ({
    ...net(inPeriod(from, to)),
    label: rangeLabel(from, to),
  });
  const previous = inPeriod(yesterday, yesterday);
  const previousNet = net(previous);
  const merged = previous.filter(({ kind }) => kind === "pr_merged");
  const yesterdayDetails: YesterdaySummary = {
    day: localDay(yesterday),
    prs: { ...previousNet, merged: merged.length },
    reviews: previous.filter(({ kind }) => kind === "review_given").length,
    mergedIds: merged.map(({ item_id }) => item_id),
  };
  return {
    today: period(today, today),
    yesterday: { ...previousNet, label: formatDay(WEEKDAY_DAY, yesterday) },
    week: period(weekStart, today),
    month: period(monthStart, today),
    yesterdayDetails,
    unsupportedZone,
    zoneLabel,
  };
}

/** Projects an instant's dashboard date onto local midnight for calendar arithmetic and labels. */
export function dashboardCalendar(timeZone?: string | null) {
  const { calendar, unsupportedZone } = dayFormatter(timeZone);
  return {
    day(date: Date) {
      if (!Number.isFinite(date.getTime())) return date;
      const parts = calendar.formatToParts(date);
      const part = (name: Intl.DateTimeFormatPartTypes) =>
        Number(parts.find(({ type }) => type === name)?.value);
      return new Date(part("year"), part("month") - 1, part("day"));
    },
    unsupportedZone,
    zoneLabel:
      !timeZone || unsupportedZone !== null
        ? `your browser’s time zone (${calendar.resolvedOptions().timeZone})`
        : timeZone,
  };
}

/** A bucket of the net chart with its axis label, full name and whether the axis labels it. */
export type ChartPoint = NetBucket & { label: string; full: string; tick: boolean };

/** Labels the buckets of the day or week chart: days by date with Mondays on the axis, weeks every second. */
export function chartPoints(buckets: readonly NetBucket[], unit: "day" | "week"): ChartPoint[] {
  return buckets.map((b, i) => {
    const start = parseDay(b.day);
    const last = i === buckets.length - 1;
    return {
      ...b,
      label: formatDay(DAY, start),
      full: unit === "day" ? formatDay(WEEKDAY_DAY, start) : `Week of ${formatDay(DAY, start)}`,
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
  events: readonly ChartEvent[],
  now: Date,
  count = THROUGHPUT_WEEKS,
): ThroughputPoint[] {
  const merged = weeklyCounts(events, ["pr_merged"], now, count);
  const reviews = weeklyCounts(events, ["review_given"], now, count);
  return merged.map((week, i) => ({
    label: formatDay(DAY, parseDay(week.day)),
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

/** The repositories represented in the chart, displayed with their GitHub owner. */
export function chartProjects(events: readonly HistoryEvent[]): string[] {
  const projects = new Map<string, string>();
  for (const event of events) {
    if (!OPENED.includes(event.kind) && !CLOSED.includes(event.kind)) continue;
    const project = historyProject(event);
    if (project) projects.set(project.toLowerCase(), project);
  }
  return Array.from(projects.values()).sort((a, b) => a.localeCompare(b));
}

/** A chart event's GitHub project as owner/repository. */
export const historyProject = (event: HistoryEvent): string | undefined => {
  const pr = parsePrRef(event.item_id);
  return pr ? `${pr.repo.owner}/${pr.repo.name}` : undefined;
};

/**
 * The pull requests behind one bar of the net chart (the bar starting at `day`), bucketed as the chart buckets
 * them; `titles` maps `owner/name#number` to the titles the snapshot knows. In event order.
 */
export function barPullRequests(
  events: readonly ChartEvent[],
  day: string,
  unit: "day" | "week",
  titles: ReadonlyMap<string, string>,
  prHref: (pr: PrRef) => string | undefined = prPath,
): { opened: BarPullRequest[]; closed: BarPullRequest[] } {
  const inBar = events
    .filter((e) => {
      const bucket = eventDate(e);
      return localDay(unit === "day" ? bucket : monday(bucket)) === day;
    })
    .sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
  const pr = (e: HistoryEvent): BarPullRequest => {
    const ref = parsePrRef(e.item_id);
    const href = ref ? prHref(ref) : undefined;
    return {
      id: e.item_id,
      title: titles.get(e.item_id.toLowerCase()) ?? null,
      url: pullRequestUrl(e.item_id),
      ...(href ? { href } : {}),
    };
  };
  return {
    opened: inBar.filter((e) => OPENED.includes(e.kind)).map(pr),
    closed: inBar
      .filter((e) => CLOSED.includes(e.kind))
      .map((e) => ({ ...pr(e), outcome: e.kind === "pr_merged" ? "merged" : "closed_unmerged" })),
  };
}
