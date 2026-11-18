import { expect, it } from "vitest";
import { flowStage } from "../../mocks/scenarios";
import type { AnalysisState } from "../../api/generated/types";
import { isIdle } from "./idle";

it("a running analysis with no flow job queued or running is idle; any stage or other state is not", () => {
  expect(isIdle({ state: "running", done: 1, total: 5 })).toBe(true);
  expect(isIdle({ state: "running", done: 1, total: 5, stage: flowStage(2, null) })).toBe(false);
  expect(isIdle({ state: "planning" })).toBe(false);
});

it("the server's idle field wins over the inference when present", () => {
  const running = { state: "running", done: 1, total: 5 } as const;
  expect(isIdle({ ...running, idle: false } as AnalysisState)).toBe(false);
  expect(isIdle({ ...running, stage: flowStage(2, null), idle: true } as AnalysisState)).toBe(true);
});
