import { expect, it } from "vitest";
import { flowStage } from "../../mocks/scenarios";
import { isIdle } from "./idle";

it("idle is what the server says: a running analysis with idle true; absent means false", () => {
  expect(isIdle({ state: "running", done: 1, total: 5, idle: true })).toBe(true);
  expect(isIdle({ state: "running", done: 1, total: 5, idle: false })).toBe(false);
  // Between two jobs the server sends no stage and idle false: work goes on.
  expect(isIdle({ state: "running", done: 1, total: 5 })).toBe(false);
  expect(isIdle({ state: "running", done: 1, total: 5, idle: false, stage: flowStage(2, null) })).toBe(false);
  expect(isIdle({ state: "planning" })).toBe(false);
});
