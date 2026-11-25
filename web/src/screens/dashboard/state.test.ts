import { describe, expect, it } from "vitest";
import type { DashboardSummary, RefreshStatus, WorkItem } from "../../api/generated/types";
import summaryJson from "../../../../fixtures/dashboard/api/summary.json";
import itemsJson from "../../../../fixtures/dashboard/api/items.json";
import scanningJson from "../../../../fixtures/dashboard/api/refresh-scanning.json";
import failedJson from "../../../../fixtures/dashboard/api/refresh-failed.json";
import { ApiError } from "../../api/client";
import {
  dashboardView,
  rankingFeedback,
  withLegacyDailyCap,
  railText,
  reviewHref,
  watchErrorText,
} from "./state";

const SUMMARY = summaryJson as unknown as DashboardSummary;
const FINISHED = SUMMARY.latest_refresh!;
const SCANNING = scanningJson as unknown as RefreshStatus;
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
describe("ranking feedback from the stored snapshot", () => {
  it("keeps the previous snapshot’s failed ranking reachable after a newer scan fails", () => {
    const failedRanking: RefreshStatus = {
      ...FINISHED,
      ranking: { ...FINISHED.ranking!, state: "failed", message: "The LLM stopped answering" },
    };
    const feedback = rankingFeedback(withLatest(FAILED), failedRanking);
    expect(feedback.retryId).toBe(FINISHED.id);
    expect(feedback.notices).toContainEqual({
      label: "Ranking failed",
      detail: "The LLM stopped answering",
    });
    expect(feedback.statusDescriptions.not_judged).toContain("Ranking failed");
  });

  it("distinguishes daily and per-refresh caps and preserves the older-prompt count", () => {
    const limited = (detail: string): RefreshStatus => ({
      ...FINISHED,
      left_provisional: 3,
      steps: FINISHED.steps.map((step) => (step.key === "ranking" ? { ...step, detail } : step)),
    });
    const daily = rankingFeedback(
      withLatest(limited("3 items left provisional: today's ranking limit (120)"), {
        older_prompt_judgements: 2,
      }),
    );
    expect(daily.notices.map((notice) => notice.label)).toContain("Daily limit reached · resets at 00:00");
    expect(daily.statusDescriptions.not_judged).toMatch(/3 items.*00:00/);
    expect(daily.notices.map(({ detail }) => detail)).toContain(
      "2 items were judged with an older ranking prompt.",
    );
    expect(daily.legacyDailyCap).toBe(true);
    const perRefresh = rankingFeedback(
      withLatest(limited("3 items left provisional: this refresh's ranking limit (48)")),
    );
    expect(perRefresh.notices.map((notice) => notice.label)).toContain("Per-refresh ranking limit");
    expect(perRefresh.legacyDailyCap).toBe(false);
  });

  it("marks legacy unjudged candidates after a completed daily-limited ranking, preserving explicit states", () => {
    const item = ITEMS.find((row) => row.id === "NC-482")!;
    const candidate = {
      ...item,
      llm: null,
      score: {
        ...item.score,
        state: "unjudged" as const,
        parts: item.score.parts.map((part) =>
          part.source === "llm" ? { ...part, status: "estimated" as const } : part,
        ),
      },
    };
    expect(withLegacyDailyCap(candidate).judgement?.state).toBe("capped");
    expect(
      withLegacyDailyCap({ ...candidate, score: { ...candidate.score, parts: [] } }).judgement,
    ).toBeUndefined();
    expect(withLegacyDailyCap(item).judgement).toBeUndefined();
    const queued = {
      ...candidate,
      judgement: { state: "queued" as const, judged_at: null, outdated_because: null },
    };
    expect(withLegacyDailyCap(queued)).toBe(queued);
  });
});

describe('dashboardView: refresh states (http-api.md "Refresh states")', () => {
  it("a finished refresh: no scan, no error, no banner", () => {
    expect(dashboardView(SUMMARY)).toEqual({
      snapshot: SUMMARY.snapshot,
      scanning: null,
      failure: null,
      stale: false,
      empty: false,
      notes: [],
    });
  });

  it("scanning: progress shown; the list is the previous snapshot", () => {
    const view = dashboardView(withLatest(SCANNING));
    expect(view).toMatchObject({ scanning: SCANNING, failure: null, stale: true });
  });

  it("a failed scan: error with the log; the list is stale", () => {
    expect(dashboardView(withLatest(FAILED))).toMatchObject({ failure: FAILED, stale: true });
  });

  it("a cancelled scan keeps the previous snapshot without an error", () => {
    expect(dashboardView(withLatest({ ...SCANNING, state: "cancelled" }))).toMatchObject({
      scanning: null,
      failure: null,
      stale: false,
    });
  });

  it("no snapshot yet; an empty snapshot", () => {
    expect(dashboardView(withLatest(null, { snapshot: null }))).toMatchObject({
      snapshot: null,
      stale: false,
      empty: false,
    });
    expect(dashboardView(withLatest(SCANNING, { snapshot: null })).stale).toBe(false);
    const counts = { ...SUMMARY.snapshot!.item_counts, total: 0, pr: 0, issue: 0, todo: 0 };
    const empty = withLatest(FINISHED, { snapshot: { ...SUMMARY.snapshot!, item_counts: counts } });
    expect(dashboardView(empty).empty).toBe(true);
  });
});

describe("dashboardView: nightly scan notes", () => {
  const titles = (summary: DashboardSummary) => dashboardView(summary).notes.map((n) => n.title);

  it("says the daily refresh waits for a time zone while it is on", () => {
    const noZone = { ...SUMMARY.schedule, time_zone: null };
    expect(titles({ ...SUMMARY, schedule: noZone })).toEqual([
      "The nightly scan at 06:00 starts once your time zone is known.",
    ]);
    expect(titles({ ...SUMMARY, schedule: { ...noZone, enabled: false } })).toEqual([]);
  });

  it("says where to set the zone once saving the browser's zone failed, not that the page saves it", () => {
    const summary = { ...SUMMARY, schedule: { ...SUMMARY.schedule, time_zone: null } };
    const [saving] = dashboardView(summary, false).notes;
    expect(saving?.text).toBe("This page saves your browser's time zone; you can change it in GitHub sync.");
    const [failed] = dashboardView(summary, true).notes;
    expect(failed).toMatchObject({
      tone: "warn",
      title: "The nightly scan at 06:00 starts once your time zone is known.",
      text: "Your browser's time zone could not be saved; set it in GitHub sync.",
    });
  });
});

describe("reviewHref (plan §F.1 PR row actions)", () => {
  const nc482 = ITEMS.find((i) => i.id === "NC-482")!;
  const start = "/pr/acme/node-client/482";

  it("head moved since the analysis: the PR main page", () => {
    expect(nc482.review?.head_moved).toBe(true);
    expect(reviewHref(nc482)).toBe(start);
  });

  it("analysis of the current head: the PR main page", () => {
    expect(reviewHref({ ...nc482, review: { ...nc482.review!, head_moved: false } })).toBe(start);
  });

  it("no analysis: the PR main page", () => {
    expect(reviewHref({ ...nc482, review: null })).toBe(start);
  });
});

describe("railText", () => {
  it("counts the PRs waiting on others and ready to merge; ready is left out when not recorded", () => {
    const snapshot = SUMMARY.snapshot!;
    expect(railText(snapshot)).toBe("More · waiting 3 · ready 1");
    expect(railText({ ...snapshot, ready_to_merge: [], merge_state_pending: null })).toBe("More · waiting 3");
  });
});

describe("watchErrorText (RW1 from a My work row)", () => {
  it("a private repository is a lasting refusal: no Try again", () => {
    const text = watchErrorText(
      new ApiError(
        422,
        "private_repository",
        "The repository is private; diffpilot supports public repositories.",
      ),
    );
    expect(text).toBe(
      "Could not watch this pull request: its repository is private, and diffpilot reads public repositories only.",
    );
    expect(text).not.toMatch(/Try again/);
  });

  it("the 20-PR limit names the way out; other failures say to try again", () => {
    expect(watchErrorText(new ApiError(409, "watch_limit_reached", "x"))).toBe(
      "Could not watch this pull request: you watch 20 already. Stop watching one under Pull requests first.",
    );
    expect(watchErrorText(new ApiError(503, "unavailable", "GitHub is not reachable."))).toBe(
      "Could not watch this pull request. Try again. GitHub is not reachable.",
    );
  });
});
