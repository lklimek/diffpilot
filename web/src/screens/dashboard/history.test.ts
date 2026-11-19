import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { HistoryEvent, HistoryKind } from "../../api/generated/types";
import {
  chartPoints,
  dailySeries,
  fullDayLabel,
  headline,
  localDay,
  throughputWeeks,
  weeklyCounts,
  weeklySeries,
  workingDay,
  yesterday,
} from "./history";

// Local times only, so the assertions hold in every time zone the tests run in.
const at = (month: number, day: number, hour = 12) => new Date(2026, month - 1, day, hour).toISOString();
const ev = (kind: HistoryKind | string, month: number, day: number, item = "acme/x#1", hour = 12) =>
  ({ kind, at: at(month, day, hour), item_id: item }) as HistoryEvent;

/** Wed 18 Nov 2026, 10:27 local. */
const NOW = new Date(2026, 10, 18, 10, 27);

const EVENTS: HistoryEvent[] = [
  ev("pr_opened", 11, 16, "acme/a#1"),
  ev("pr_merged", 11, 17, "acme/a#2"),
  ev("pr_merged", 11, 17, "acme/a#3"),
  ev("review_given", 11, 17, "acme/b#9"),
  ev("pr_closed_unmerged", 11, 18, "acme/a#4"),
  // Weekend: counted on the Friday before.
  ev("pr_opened", 11, 14, "acme/a#5"),
  ev("pr_merged", 11, 15, "acme/a#6"),
  // Last week, Mon 9 Nov.
  ev("pr_opened", 11, 9, "acme/a#7"),
  // A kind this client does not know is ignored (tolerant reader).
  ev("issue_closed", 11, 17, "acme/a#8"),
];

describe("workingDay", () => {
  it("keeps a weekday and moves Saturday and Sunday to the Friday before", () => {
    expect(localDay(workingDay(new Date(2026, 10, 18, 23, 59)))).toBe("2026-11-18");
    expect(localDay(workingDay(new Date(2026, 10, 14, 1)))).toBe("2026-11-13");
    expect(localDay(workingDay(new Date(2026, 10, 15, 23)))).toBe("2026-11-13");
  });
});

describe("headline (pull requests only, closed − opened)", () => {
  it("counts this week from Monday to now, the previous working day, last week and four weeks", () => {
    const h = headline(EVENTS, NOW);
    expect(h.week).toEqual({ label: "Mon–Wed", closed: 3, opened: 1, net: 2 });
    expect(h.yesterday).toEqual({ label: "Tue 17 Nov", closed: 2, opened: 0, net: 2 });
    // Mon 9 – Sun 15 Nov: opened Mon 9 and Sat 14, merged Sun 15.
    expect(h.lastWeek).toEqual({ label: "9–13 Nov", closed: 1, opened: 2, net: -1 });
    expect(h.fourWeeks).toEqual({ net: 1, before: 0 });
  });

  it("on a Monday, yesterday is the Friday with the weekend after it", () => {
    const h = headline(EVENTS, new Date(2026, 10, 16, 9));
    expect(h.yesterday).toEqual({ label: "Fri 13 Nov", closed: 1, opened: 1, net: 0 });
    expect(h.week.label).toBe("Mon");
    expect(headline([], new Date(2026, 11, 2, 9)).lastWeek.label).toBe("23–27 Nov");
    expect(headline([], new Date(2026, 11, 9, 9)).lastWeek.label).toBe("30 Nov – 4 Dec");
  });

  it("counts the four weeks before the last twenty working days", () => {
    const h = headline([ev("pr_merged", 10, 21), ev("pr_merged", 9, 23), ev("pr_opened", 9, 22)], NOW);
    // Window: Thu 22 Oct – Wed 18 Nov; before: Thu 24 Sep – Wed 21 Oct.
    expect(h.fourWeeks).toEqual({ net: 0, before: 1 });
  });
});

describe("dailySeries", () => {
  it("lists the last twenty working days with net and running total; today is partial", () => {
    const days = dailySeries(EVENTS, NOW);
    expect(days).toHaveLength(20);
    expect(days[0]!.day).toBe("2026-10-22");
    expect(days.at(-1)).toEqual({
      day: "2026-11-18",
      closed: 1,
      opened: 0,
      net: 1,
      running: 1,
      partial: true,
    });
    expect(days.find((d) => d.day === "2026-11-13")).toMatchObject({ closed: 1, opened: 1, net: 0 });
    expect(days.find((d) => d.day === "2026-11-17")).toMatchObject({ closed: 2, net: 2, partial: false });
    expect(days.some((d) => d.day === "2026-11-14" || d.day === "2026-11-15")).toBe(false);
    let running = 0;
    for (const d of days) {
      running += d.net;
      expect(d.running).toBe(running);
      expect(d.net).toBe(d.closed - d.opened);
    }
  });

  it("on a Saturday the Friday is the partial last day and holds the Saturday", () => {
    const days = dailySeries([ev("pr_merged", 11, 21)], new Date(2026, 10, 21, 10));
    expect(days.at(-1)).toMatchObject({ day: "2026-11-20", closed: 1, partial: true });
  });
});

describe("weeklySeries", () => {
  it("lists twelve weeks from Monday; the current week is partial", () => {
    const weeks = weeklySeries(EVENTS, NOW);
    expect(weeks).toHaveLength(12);
    expect(weeks[0]!.day).toBe("2026-08-31");
    expect(weeks.at(-1)).toMatchObject({ day: "2026-11-16", closed: 3, opened: 1, net: 2, partial: true });
    expect(weeks.at(-2)).toMatchObject({ day: "2026-11-09", closed: 1, opened: 2, net: -1, partial: false });
    expect(weeks.at(-1)!.running).toBe(1);
  });
});

describe("weeklyCounts", () => {
  it("counts one kind per week for the throughput charts", () => {
    expect(weeklyCounts(EVENTS, ["pr_merged"], NOW, 3)).toEqual([
      { day: "2026-11-02", count: 0, partial: false },
      { day: "2026-11-09", count: 1, partial: false },
      { day: "2026-11-16", count: 2, partial: true },
    ]);
    expect(weeklyCounts(EVENTS, ["review_given"], NOW, 1)).toEqual([
      { day: "2026-11-16", count: 1, partial: true },
    ]);
  });
});

describe("yesterday", () => {
  it("sums pull requests and reviews of the previous working day and names the merged ones", () => {
    expect(yesterday(EVENTS, NOW)).toEqual({
      day: "2026-11-17",
      prs: { closed: 2, opened: 0, net: 2, merged: 2 },
      reviews: 1,
      mergedIds: ["acme/a#2", "acme/a#3"],
    });
  });
});

describe("chart labels", () => {
  it("names days and weeks; Mondays (days) and every second week are on the axis, the last always", () => {
    const days = chartPoints(dailySeries(EVENTS, NOW), "day");
    expect(days.at(-1)).toMatchObject({ label: "18 Nov", full: "Wed 18 Nov", tick: true, partial: true });
    expect(days.find((d) => d.day === "2026-11-16")).toMatchObject({ full: "Mon 16 Nov", tick: true });
    expect(days.find((d) => d.day === "2026-11-17")!.tick).toBe(false);
    const weeks = chartPoints(weeklySeries(EVENTS, NOW), "week");
    expect(weeks.map((w) => w.tick)).toEqual([
      false,
      true,
      false,
      true,
      false,
      true,
      false,
      true,
      false,
      true,
      false,
      true,
    ]);
    expect(weeks.at(-1)).toMatchObject({ label: "16 Nov", full: "Week of 16 Nov" });
    expect(fullDayLabel("2026-11-17")).toBe("Tue 17 Nov");
  });

  it("throughput weeks: merged pull requests and reviews given", () => {
    expect(throughputWeeks(EVENTS, NOW, 2)).toEqual([
      { label: "9 Nov", merged: 1, reviews: 0, partial: false },
      { label: "16 Nov", merged: 2, reviews: 1, partial: true },
    ]);
  });
});

describe("bucketing across the end of summer time (Europe/Warsaw, Sun 25 Oct 2026)", () => {
  // Node applies a changed TZ at once; the first assertion proves it did, so the case cannot pass vacuously.
  const zone = process.env.TZ;
  beforeAll(() => {
    process.env.TZ = "Europe/Warsaw";
  });
  afterAll(() => {
    if (zone === undefined) delete process.env.TZ;
    else process.env.TZ = zone;
  });
  const utc = (kind: HistoryKind, iso: string, item: string) =>
    ({ kind, at: iso, item_id: item }) as HistoryEvent;

  it("counts by local day and week on the 25-hour Sunday and the Monday after it", () => {
    expect(new Date("2026-10-24T12:00:00Z").getTimezoneOffset()).toBe(-120);
    expect(new Date("2026-10-26T12:00:00Z").getTimezoneOffset()).toBe(-60);
    const events = [
      // Sun 25 Oct 23:30 CET: a weekend event, counted on Fri 23 Oct.
      utc("pr_merged", "2026-10-25T22:30:00Z", "acme/a#1"),
      // Mon 26 Oct 00:30 CET (still Sunday in UTC): Monday, in the week of 26 Oct.
      utc("pr_opened", "2026-10-25T23:30:00Z", "acme/a#2"),
      // Sun 25 Oct 02:30 CEST, before the clocks went back: Friday too.
      utc("pr_merged", "2026-10-25T00:30:00Z", "acme/a#3"),
    ];
    const now = new Date("2026-10-27T09:00:00Z");
    const days = dailySeries(events, now, 3);
    expect(days.map((d) => [d.day, d.closed, d.opened])).toEqual([
      ["2026-10-23", 2, 0],
      ["2026-10-26", 0, 1],
      ["2026-10-27", 0, 0],
    ]);
    const weeks = weeklySeries(events, now, 2);
    expect(weeks.map((w) => [w.day, w.closed, w.opened])).toEqual([
      ["2026-10-19", 2, 0],
      ["2026-10-26", 0, 1],
    ]);
    expect(weeklyCounts(events, ["pr_merged"], now, 2).map((w) => w.count)).toEqual([2, 0]);
  });
});
