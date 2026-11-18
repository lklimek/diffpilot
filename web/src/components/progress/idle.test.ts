import { expect, it } from "vitest";
import { flowStage } from "../../mocks/scenarios";
import { isIdle } from "./idle";

it("a running analysis with no flow job queued or running is idle; any stage or other state is not", () => {
  expect(isIdle({ state: "running", done: 1, total: 5 })).toBe(true);
  expect(isIdle({ state: "running", done: 1, total: 5, stage: flowStage(2, null) })).toBe(false);
  expect(isIdle({ state: "planning" })).toBe(false);
});
