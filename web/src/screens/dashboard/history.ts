// History bucketing of the dashboard charts (plan §C.1, §F.3): the snapshot's dated pull-request and review events
// counted by the browser's local working day and week. Pull requests only (OD16): closed = merged or closed
// unmerged, opened = opened; reviews given are counted apart and never enter the net.
import type { AwaitingItem, HistoryEvent, HistoryKind } from "../../api/generated/types";

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

/** Values of the headline card. */
export interface Headline {
  /** This week from Monday to now. */
  week: NetCount & { from: string };
  /** The previous working day (with the weekend after it). */
  yesterday: NetCount & { day: string };
  /** Last week, Monday to Friday (its weekend included). */
  lastWeek: NetCount & { from: string; to: string };
  /** The last twenty working days, and the twenty before them. */
  fourWeeks: { net: number; before: number };
}

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

/** Headline values: this week, yesterday, last week and the last four weeks against the four before. */
export function headline(events: readonly HistoryEvent[], now: Date): Headline {
  const weekStart = monday(now);
  const tomorrow = addDays(workingDay(now), 1);
  const yesterdayStart = previousWorkingDay(workingDay(now));
  const lastWeekStart = addDays(weekStart, -7);
  const window = workingDays(now, 2 * CHART_DAYS);
  const recentStart = window[CHART_DAYS]!;
  return {
    week: { from: localDay(weekStart), ...netBetween(events, weekStart, tomorrow) },
    yesterday: {
      day: localDay(yesterdayStart),
      ...netBetween(events, yesterdayStart, addDays(yesterdayStart, 1)),
    },
    lastWeek: {
      from: localDay(lastWeekStart),
      to: localDay(addDays(lastWeekStart, 4)),
      ...netBetween(events, lastWeekStart, weekStart),
    },
    fourWeeks: {
      net: netBetween(events, recentStart, tomorrow).net,
      before: netBetween(events, window[0]!, recentStart).net,
    },
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

/** Wait buckets of the review-queue age chart, in whole local days. */
const AGE_BUCKETS: readonly { label: string; max: number }[] = [
  { label: "≤ 2 d", max: 2 },
  { label: "3–7 d", max: 7 },
  { label: "8–14 d", max: 14 },
  { label: "> 14 d", max: Infinity },
];

/** How long the user's pull requests have waited on others, bucketed by whole local days. */
export function queueAge(awaiting: readonly AwaitingItem[], now: Date): { label: string; count: number }[] {
  const today = midnight(now).getTime();
  const counts = AGE_BUCKETS.map((b) => ({ label: b.label, count: 0 }));
  for (const item of awaiting) {
    const days = Math.round((today - midnight(new Date(item.waiting_since)).getTime()) / 86_400_000);
    counts[AGE_BUCKETS.findIndex((b) => days <= b.max)]!.count += 1;
  }
  return counts;
}
