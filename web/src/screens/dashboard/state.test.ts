import { describe, expect, it } from "vitest";
import type { DashboardSummary, RefreshStatus, WorkItem } from "../../api/generated/types";
import summaryJson from "../../../../fixtures/dashboard/api/summary.json";
import itemsJson from "../../../../fixtures/dashboard/api/items.json";
import scanningJson from "../../../../fixtures/dashboard/api/refresh-scanning.json";
import rankingJson from "../../../../fixtures/dashboard/api/refresh-ranking.json";
import failedJson from "../../../../fixtures/dashboard/api/refresh-failed.json";
import { dashboardView, reviewHref } from "./state";

const SUMMARY = summaryJson as unknown as DashboardSummary;
const FINISHED = SUMMARY.latest_refresh!;
const SCANNING = scanningJson as unknown as RefreshStatus;
const RANKING = rankingJson as unknown as RefreshStatus;
const FAILED = failedJson as unknown as RefreshStatus;
const ITEMS = (itemsJson as unknown as { items: WorkItem[] }).items;

const withLatest = (
  latest: RefreshStatus | null,
  extra: Partial<DashboardSummary> = {},
): DashboardSummary => ({
  ...SUMMARY,
  latest_refresh: latest,
  ...extra,
});
/** The ranking refresh's own (provisional) snapshot. */
const PROVISIONAL_SNAPSHOT = {
  ...SUMMARY.snapshot!,
  refresh_id: RANKING.id,
  scanned_at: "2026-11-18T09:24:00Z",
  ranked_at: null,
};
const ranked = (
  ranking: Partial<NonNullable<RefreshStatus["ranking"]>>,
  state: RefreshStatus["state"] = "ranking",
) =>
  withLatest(
    { ...RANKING, state, ranking: { ...RANKING.ranking!, ...ranking } },
    { snapshot: PROVISIONAL_SNAPSHOT },
  );

describe('dashboardView: refresh states (http-api.md "Refresh states")', () => {
  it("a finished refresh: no scan, no error, no banner", () => {
    expect(dashboardView(SUMMARY, undefined)).toEqual({
      snapshot: SUMMARY.snapshot,
      scanning: null,
      failure: null,
      ranking: null,
      stale: false,
      empty: false,
      notes: [],
    });
  });

  it("scanning: progress shown; the list is the previous snapshot", () => {
    const view = dashboardView(withLatest(SCANNING), undefined);
    expect(view).toMatchObject({ scanning: SCANNING, failure: null, ranking: null, stale: true });
  });

  it("each live ranking state is a banner of the snapshot's refresh", () => {
    for (const state of ["running", "waiting_for_analysis", "paused"] as const) {
      const view = dashboardView(ranked({ state }), undefined);
      expect(view.ranking).toEqual({
        status: { ...RANKING.ranking, state },
        refreshId: RANKING.id,
        scannedAt: "2026-11-18T09:24:00Z",
      });
      expect(view.stale).toBe(false);
    }
  });

  it("a stopped ranking keeps its banner (scan result with estimates, Retry ranking)", () => {
    for (const state of ["failed", "interrupted", "cancelled"] as const) {
      expect(dashboardView(ranked({ state }, "finished"), undefined).ranking).toMatchObject({
        status: { state },
        refreshId: RANKING.id,
      });
    }
  });

  it("a finished ranking shows no banner", () => {
    expect(dashboardView(ranked({ state: "finished" }, "finished"), undefined).ranking).toBeNull();
  });

  it("a superseded ranking: only the new scan shows; after that scan failed the old ranking can run again", () => {
    const superseded: RefreshStatus = {
      ...RANKING,
      state: "finished",
      ranking: { ...RANKING.ranking!, state: "cancelled" },
    };
    const summary = withLatest(SCANNING, { snapshot: PROVISIONAL_SNAPSHOT });
    expect(dashboardView(summary, superseded)).toMatchObject({ ranking: null, scanning: SCANNING });
    const failed = dashboardView({ ...summary, latest_refresh: FAILED }, superseded);
    expect(failed.ranking).toMatchObject({ status: { state: "cancelled" }, refreshId: RANKING.id });
    expect(failed.failure).toBe(FAILED);
    // Without the snapshot refresh's own status its ranking is unknown: no banner.
    expect(dashboardView({ ...summary, latest_refresh: FAILED }, undefined).ranking).toBeNull();
  });

  it("a failed scan: error with the log; the list is stale", () => {
    expect(dashboardView(withLatest(FAILED), undefined)).toMatchObject({ failure: FAILED, stale: true });
  });

  it("a cancelled scan keeps the previous snapshot without an error", () => {
    expect(dashboardView(withLatest({ ...SCANNING, state: "cancelled" }), undefined)).toMatchObject({
      scanning: null,
      failure: null,
      stale: false,
    });
  });

  it("no snapshot yet; an empty snapshot", () => {
    expect(dashboardView(withLatest(null, { snapshot: null }), undefined)).toMatchObject({
      snapshot: null,
      stale: false,
      empty: false,
    });
    expect(dashboardView(withLatest(SCANNING, { snapshot: null }), undefined).stale).toBe(false);
    const counts = { ...SUMMARY.snapshot!.item_counts, total: 0, pr: 0, issue: 0, todo: 0 };
    const empty = withLatest(FINISHED, { snapshot: { ...SUMMARY.snapshot!, item_counts: counts } });
    expect(dashboardView(empty, undefined).empty).toBe(true);
  });
});

describe("dashboardView: schedule and limit notes", () => {
  const titles = (summary: DashboardSummary) => dashboardView(summary, undefined).notes.map((n) => n.title);

  it("says the daily refresh waits for a time zone while it is on", () => {
    const noZone = { ...SUMMARY.schedule, time_zone: null };
    expect(titles({ ...SUMMARY, schedule: noZone })).toEqual([
      "The daily refresh at 06:00 starts once your time zone is known.",
    ]);
    expect(titles({ ...SUMMARY, schedule: { ...noZone, enabled: false } })).toEqual([]);
  });

  it("says where to set the zone once saving the browser's zone failed, not that the page saves it", () => {
    const summary = { ...SUMMARY, schedule: { ...SUMMARY.schedule, time_zone: null } };
    const [saving] = dashboardView(summary, undefined, false).notes;
    expect(saving?.text).toBe("This page saves your browser's time zone; you can change it in Settings.");
    const [failed] = dashboardView(summary, undefined, true).notes;
    expect(failed).toMatchObject({
      tone: "warn",
      title: "The daily refresh at 06:00 starts once your time zone is known.",
      text: "Your browser's time zone could not be saved; set it in Settings.",
    });
  });

  /** `FINISHED` with `left` items left provisional and the server's ranking step detail. */
  const limitedBy = (left: number, detail: string | null, state: "finished" | "failed" = "finished") => ({
    ...FINISHED,
    left_provisional: left,
    steps: FINISHED.steps.map((step) => (step.key === "ranking" ? { ...step, detail } : step)),
    ranking: { ...FINISHED.ranking!, state },
  });

  it("names the ranking limit the server applied, and counts items of an older prompt", () => {
    const perRefresh = withLatest(
      limitedBy(18, "18 items left provisional: this refresh's ranking limit (48)"),
      { older_prompt_judgements: 4 },
    );
    expect(titles(perRefresh)).toEqual([
      "18 items left provisional: this refresh's ranking limit (48).",
      "4 items judged with an older ranking prompt.",
    ]);
    expect(titles(withLatest(limitedBy(1, "1 item left provisional: today's ranking limit (120)")))).toEqual([
      "1 item left provisional: today's ranking limit (120).",
    ]);
  });

  it("names no limit when the ranking failed or the server named none", () => {
    expect(titles(withLatest(limitedBy(3, "3 items could not be judged (PLAT-1)", "failed")))).toEqual([
      "3 items left provisional.",
    ]);
    expect(titles(withLatest(limitedBy(1, null)))).toEqual(["1 item left provisional."]);
  });
});

describe("reviewHref (plan §F.1 PR row actions)", () => {
  const nc482 = ITEMS.find((i) => i.id === "NC-482")!;
  const start = `/review?pr=${encodeURIComponent("https://github.com/acme/node-client/pull/482")}`;

  it("head moved since the analysis: the review start page with the PR (it says the PR moved)", () => {
    expect(nc482.review?.head_moved).toBe(true);
    expect(reviewHref(nc482)).toBe(start);
  });

  it("analysis of the current head: the review itself", () => {
    expect(reviewHref({ ...nc482, review: { ...nc482.review!, head_moved: false } })).toBe(
      `/analyses/${nc482.review!.analysis_id}`,
    );
  });

  it("no analysis: the review start page with the PR", () => {
    expect(reviewHref({ ...nc482, review: null })).toBe(start);
  });
});
