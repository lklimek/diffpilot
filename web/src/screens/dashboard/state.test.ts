import { describe, expect, it } from "vitest";
import type { DashboardSummary, RefreshStatus, ScheduledRun, WorkItem } from "../../api/generated/types";
import summaryJson from "../../../../fixtures/dashboard/api/summary.json";
import itemsJson from "../../../../fixtures/dashboard/api/items.json";
import scanningJson from "../../../../fixtures/dashboard/api/refresh-scanning.json";
import rankingJson from "../../../../fixtures/dashboard/api/refresh-ranking.json";
import failedJson from "../../../../fixtures/dashboard/api/refresh-failed.json";
import { dashboardView, filterItems, reviewAction } from "./state";

const SUMMARY = summaryJson as unknown as DashboardSummary;
const FINISHED = SUMMARY.latest_refresh!;
const SCANNING = scanningJson as unknown as RefreshStatus;
const RANKING = rankingJson as unknown as RefreshStatus;
const FAILED = failedJson as unknown as RefreshStatus;
const ITEMS = (itemsJson as unknown as { items: WorkItem[] }).items;
/** 10:27 UTC on the fixture day: GitHub's limit resets at 10:41. */
const NOW = Date.parse("2026-11-18T10:27:00Z");

const withLatest = (
  latest: RefreshStatus | null,
  extra: Partial<DashboardSummary> = {},
): DashboardSummary => ({
  ...SUMMARY,
  latest_refresh: latest,
  ...extra,
});
/** The ranking refresh holding its own (provisional) snapshot. */
const ranked = (
  ranking: Partial<NonNullable<RefreshStatus["ranking"]>>,
  state: RefreshStatus["state"] = "ranking",
) =>
  withLatest(
    { ...RANKING, state, ranking: { ...RANKING.ranking!, ...ranking } },
    { snapshot: { ...SUMMARY.snapshot!, refresh_id: RANKING.id, ranked_at: null } },
  );

describe('dashboardView: refresh states (http-api.md "Refresh states")', () => {
  it("a finished refresh: ready to refresh, scan info of the snapshot, no banners", () => {
    const view = dashboardView(SUMMARY, undefined, NOW);
    expect(view).toMatchObject({
      scanning: null,
      failure: null,
      ranking: null,
      refreshButton: { state: "ready" },
      stale: false,
      empty: false,
      notes: [],
    });
    expect(view.scanInfo).toEqual({
      trigger: "manual",
      scannedAt: "2026-11-18T08:14:00Z",
      rankedAt: "2026-11-18T08:15:04Z",
      judged: 6,
      reused: 31,
    });
  });

  it("scanning: progress shown, Refresh disabled (a second start is refused), the list is the previous snapshot", () => {
    const view = dashboardView(withLatest(SCANNING), undefined, NOW);
    expect(view.scanning).toBe(SCANNING);
    expect(view.refreshButton).toEqual({ state: "scanning" });
    expect(view.stale).toBe(true);
    expect(view.ranking).toBeNull();
    // The snapshot is still the 08:12 refresh's, known only by its own status.
    expect(view.scanInfo).toMatchObject({ scannedAt: "2026-11-18T08:14:00Z", trigger: null });
  });

  it("ranking: k of n with Refresh allowed (a new refresh supersedes the ranking)", () => {
    const view = dashboardView(ranked({}), undefined, NOW);
    expect(view.ranking).toEqual({ kind: "running", judged: 10, total: 14 });
    expect(view.refreshButton).toEqual({ state: "ready" });
    expect(view.stale).toBe(false);
  });

  it("waiting for the review analysis and paused are their own banners", () => {
    expect(dashboardView(ranked({ state: "waiting_for_analysis" }), undefined, NOW).ranking).toEqual({
      kind: "waiting_for_analysis",
      judged: 10,
      total: 14,
    });
    expect(
      dashboardView(ranked({ state: "paused", resumes_at: "2026-11-18T11:00:00Z" }), undefined, NOW).ranking,
    ).toEqual({ kind: "paused", judged: 10, total: 14, resumesAt: "2026-11-18T11:00:00Z" });
  });

  it("a stopped ranking keeps the scan result with estimates and offers Rank again", () => {
    for (const state of ["failed", "interrupted", "cancelled"] as const) {
      const view = dashboardView(ranked({ state, message: "boom" }, "finished"), undefined, NOW);
      expect(view.ranking).toEqual({
        kind: "stopped",
        state,
        message: "boom",
        refreshId: RANKING.id,
        canRankAgain: true,
      });
    }
  });

  it("a finished ranking shows no banner", () => {
    expect(dashboardView(ranked({ state: "finished" }, "finished"), undefined, NOW).ranking).toBeNull();
  });

  it("a superseded ranking: while the new refresh scans only the scan shows; Rank again waits for it", () => {
    const superseded: RefreshStatus = {
      ...RANKING,
      state: "finished",
      ranking: { ...RANKING.ranking!, state: "cancelled" },
    };
    const summary = withLatest(SCANNING, {
      snapshot: { ...SUMMARY.snapshot!, refresh_id: RANKING.id, ranked_at: null },
    });
    const view = dashboardView(summary, superseded, NOW);
    expect(view.ranking).toBeNull();
    expect(view.scanning).toBe(SCANNING);
    // The new scan failed: the superseded snapshot stays current and its ranking can run again.
    const failed = dashboardView({ ...summary, latest_refresh: FAILED }, superseded, NOW);
    expect(failed.ranking).toMatchObject({ kind: "stopped", state: "cancelled", canRankAgain: true });
    expect(failed.scanInfo).toMatchObject({ trigger: "manual", judged: 10, reused: 27 });
  });

  it("a failed scan: error with the log, Refresh blocked until GitHub's reset, the list is stale", () => {
    const view = dashboardView(withLatest(FAILED), undefined, NOW);
    expect(view.failure).toBe(FAILED);
    expect(view.refreshButton).toEqual({ state: "blocked", retryAt: "2026-11-18T10:41:00Z" });
    expect(view.stale).toBe(true);
    // After the reset time a new refresh may start.
    expect(
      dashboardView(withLatest(FAILED), undefined, Date.parse("2026-11-18T10:41:00Z")).refreshButton,
    ).toEqual({
      state: "ready",
    });
    const noRetry = { ...FAILED, failure: { ...FAILED.failure!, retry_at: null } };
    expect(dashboardView(withLatest(noRetry), undefined, NOW).refreshButton).toEqual({ state: "ready" });
  });

  it("a cancelled scan keeps the previous snapshot without an error", () => {
    const view = dashboardView(withLatest({ ...SCANNING, state: "cancelled" }), undefined, NOW);
    expect(view).toMatchObject({
      scanning: null,
      failure: null,
      stale: false,
      refreshButton: { state: "ready" },
    });
  });

  it("no snapshot yet; an empty snapshot", () => {
    expect(dashboardView(withLatest(null, { snapshot: null }), undefined, NOW)).toMatchObject({
      snapshot: null,
      scanInfo: null,
      empty: false,
    });
    const counts = {
      ...SUMMARY.snapshot!.item_counts,
      total: 0,
      pr: 0,
      issue: 0,
      todo: 0,
      p0: 0,
      p1: 0,
      p2: 0,
      p3: 0,
    };
    const empty = withLatest(FINISHED, { snapshot: { ...SUMMARY.snapshot!, item_counts: counts } });
    expect(dashboardView(empty, undefined, NOW).empty).toBe(true);
  });
});

describe("dashboardView: schedule and limit notes", () => {
  const schedule = (last_run: ScheduledRun | null, time_zone: string | null = "Europe/Warsaw") => ({
    ...SUMMARY.schedule,
    time_zone,
    last_run,
  });
  const titles = (summary: DashboardSummary) =>
    dashboardView(summary, undefined, NOW).notes.map((n) => n.title);

  it("says the daily refresh waits for a time zone", () => {
    expect(titles({ ...SUMMARY, schedule: schedule(null, null) })).toEqual([
      "The daily refresh at 06:00 starts once your time zone is known.",
    ]);
    expect(titles({ ...SUMMARY, schedule: { ...schedule(null, null), enabled: false } })).toEqual([]);
  });

  it("shows a scheduled run that needs sign-in or failed until a later refresh succeeds", () => {
    const signIn = {
      at: "2026-11-18T05:00:00Z",
      outcome: "needs_sign_in",
      reason: "GitHub refused the stored sign-in.",
    } as const;
    // The finished refresh started at 08:12, after the 06:00 run: nothing to show.
    expect(titles({ ...SUMMARY, schedule: schedule(signIn) })).toEqual([]);
    const before = withLatest(
      { ...FINISHED, started_at: "2026-11-17T08:00:00Z" },
      { schedule: schedule(signIn) },
    );
    expect(dashboardView(before, undefined, NOW).notes).toEqual([
      {
        id: "schedule",
        tone: "warn",
        title: "Automatic refresh at 06:00 needs you to sign in again.",
        text: "GitHub refused the stored sign-in.",
      },
    ]);
    // A later refresh that failed does not clear it.
    const failedLater = withLatest(FAILED, {
      schedule: schedule({ ...signIn, outcome: "failed", reason: "GitHub was unavailable." }),
    });
    expect(titles(failedLater)).toEqual(["Automatic refresh at 06:00 failed."]);
    expect(titles(withLatest(null, { schedule: schedule(signIn) }))).toEqual([
      "Automatic refresh at 06:00 needs you to sign in again.",
    ]);
  });

  it("counts items left provisional by the ranking limit and items of an older prompt", () => {
    const limited = withLatest({ ...FINISHED, left_provisional: 1 }, { older_prompt_judgements: 4 });
    expect(titles(limited)).toEqual([
      "1 item left provisional: daily ranking limit.",
      "4 items judged with an older ranking prompt.",
    ]);
  });
});

describe("filterItems (PRs / issues / both; TODOs always listed)", () => {
  it("keeps TODOs with every kind filter", () => {
    const kinds = (filter: "pr" | "issue" | "both") => new Set(filterItems(ITEMS, filter).map((i) => i.kind));
    expect(kinds("pr")).toEqual(new Set(["pr", "todo"]));
    expect(kinds("issue")).toEqual(new Set(["issue", "todo"]));
    expect(kinds("both")).toEqual(new Set(["pr", "issue", "todo"]));
    expect(filterItems(ITEMS, "both")).toHaveLength(ITEMS.length);
  });
});

describe("reviewAction (plan §F.1 PR row actions)", () => {
  const nc482 = ITEMS.find((i) => i.id === "NC-482")!;
  const url = encodeURIComponent(nc482.url!);

  it("head moved since the analysis: the review start page with the PR (moved banner)", () => {
    expect(reviewAction(nc482)).toEqual({ kind: "moved", href: `/review?pr=${url}`, label: "Review" });
  });

  it("analysis of the current head: the review itself (Resume when the user has state there)", () => {
    const current = { ...nc482, review: { ...nc482.review!, head_moved: false } };
    expect(reviewAction(current)).toEqual({
      kind: "found",
      href: `/analyses/${nc482.review!.analysis_id}`,
      label: "Resume review",
    });
    expect(reviewAction({ ...current, review: { ...current.review, has_my_state: false } })?.label).toBe(
      "Open review",
    );
  });

  it("no analysis: the review start page with the PR; issues and TODOs have none", () => {
    expect(reviewAction({ ...nc482, review: null })).toEqual({
      kind: "new",
      href: `/review?pr=${url}`,
      label: "Review",
    });
    expect(reviewAction(ITEMS.find((i) => i.kind === "issue")!)).toBeNull();
    expect(reviewAction(ITEMS.find((i) => i.kind === "todo")!)).toBeNull();
  });
});
