// @vitest-environment node
import { describe, expect, it } from "vitest";
import type { AnalysisState, AnalysisSummary, ResolvePrResponse } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { analysisHref, homeOutcome, startsAtOnce } from "./outcome";

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

describe("homeOutcome (brief §4.1)", () => {
  it("no analysis or a cancelled one: analyze", () => {
    expect(homeOutcome(resolved(null))).toEqual({ kind: "analyze" });
    expect(homeOutcome(resolved({ state: "cancelled" }))).toEqual({ kind: "analyze" });
    expect(homeOutcome(resolved({ state: "cancelled" }, { head_moved: true }))).toEqual({ kind: "analyze" });
  });

  it("finished at the current head: resume", () => {
    expect(homeOutcome(resolved({ state: "finished", total: 5 }))).toMatchObject({ kind: "resume" });
  });

  it("head moved: the head-moved choice, whatever the old analysis's state", () => {
    for (const state of [{ state: "finished", total: 5 }, { state: "planning" }] as AnalysisState[]) {
      expect(homeOutcome(resolved(state, { head_moved: true }))).toMatchObject({ kind: "head_moved" });
    }
  });

  it("planning, running or paused: in progress; failed: failed", () => {
    for (const state of [
      { state: "planning" },
      { state: "running", done: 1, total: 2 },
      { state: "paused", resumes_at: null },
    ] as AnalysisState[]) {
      expect(homeOutcome(resolved(state))).toMatchObject({ kind: "in_progress" });
    }
    expect(homeOutcome(resolved({ state: "failed", flow: null, message: "x" }))).toMatchObject({
      kind: "failed",
    });
  });

  it("starts at once only when there is nothing to resume and commenting works", () => {
    expect(startsAtOnce(resolved(null))).toBe(true);
    expect(startsAtOnce(resolved(null, { commenting_available: false }))).toBe(false);
    expect(startsAtOnce(resolved({ state: "finished", total: 5 }))).toBe(false);
  });

  it("analysisHref: the review when finished, else the progress screen", () => {
    expect(analysisHref({ ...analysis, state: { state: "finished", total: 5 } })).toBe(
      `/analyses/${analysis.id}`,
    );
    expect(analysisHref({ ...analysis, state: { state: "planning" } })).toBe(
      `/analyses/${analysis.id}/progress`,
    );
  });
});
