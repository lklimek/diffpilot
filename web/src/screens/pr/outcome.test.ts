// @vitest-environment node
import { describe, expect, it } from "vitest";
import type { AnalysisState, AnalysisSummary, ResolvePrResponse } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { analysisHref, reviewOutcome } from "./outcome";

const seed = seedFor("default", "pr-resolve") as ResolvePrResponse;
const analysis = seedFor("default", "analysis") as AnalysisSummary;
const resolved = (
  state: AnalysisState | null,
  extra: Partial<ResolvePrResponse> = {},
): ResolvePrResponse => ({
  ...seed,
  analysis: state && { ...analysis, state },
  ...extra,
});

describe("reviewOutcome (brief §4.1)", () => {
  it("no analysis or a cancelled one: analyze", () => {
    expect(reviewOutcome(resolved(null))).toEqual({ kind: "analyze" });
    expect(reviewOutcome(resolved({ state: "cancelled" }))).toEqual({ kind: "analyze" });
    expect(reviewOutcome(resolved({ state: "cancelled" }, { head_moved: true }))).toEqual({
      kind: "analyze",
    });
  });

  it("finished at the current head: resume", () => {
    expect(reviewOutcome(resolved({ state: "finished", total: 5 }))).toMatchObject({ kind: "resume" });
  });

  it("head moved: the head-moved choice, whatever the old analysis's state", () => {
    for (const state of [{ state: "finished", total: 5 }, { state: "planning" }] as AnalysisState[]) {
      expect(reviewOutcome(resolved(state, { head_moved: true }))).toMatchObject({ kind: "head_moved" });
    }
  });

  it("planning, running or paused: in progress; failed: failed", () => {
    for (const state of [
      { state: "planning" },
      { state: "running", done: 1, total: 2 },
      { state: "paused", resumes_at: null, done: 0, total: null },
    ] as AnalysisState[]) {
      expect(reviewOutcome(resolved(state))).toMatchObject({ kind: "in_progress" });
    }
    expect(reviewOutcome(resolved({ state: "failed", flow: null, message: "x" }))).toMatchObject({
      kind: "failed",
    });
  });

  it("analysisHref: the review when finished, else the progress screen", () => {
    expect(analysisHref({ ...analysis, state: { state: "finished", total: 5 } })).toBe(
      `/pr/acme/node-client/482/analyses/${analysis.id}`,
    );
    expect(analysisHref({ ...analysis, state: { state: "planning" } })).toBe(
      `/pr/acme/node-client/482/analyses/${analysis.id}/progress`,
    );
  });

  it("a failed analysis with a readable review opens the Tour, which shows the failure and Retry", () => {
    const failed = { state: "failed", flow: null, message: "Boom." } as const;
    expect(analysisHref({ ...analysis, review_ready: true, state: failed })).toBe(
      `/pr/acme/node-client/482/analyses/${analysis.id}`,
    );
    expect(analysisHref({ ...analysis, review_ready: false, state: failed })).toBe(
      `/pr/acme/node-client/482/analyses/${analysis.id}/progress`,
    );
  });
});
