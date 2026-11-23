// Self-test of the stack pool's rule (support/fixtures.ts), no stack: a file's tests share one stack started with the
// file's options; another file gets its own; a test asking for other options than its file's stack is an error.
import { expect, test } from "@playwright/test";
import { reusesStack } from "../support/fixtures";

const running = { file: "/e2e/tests/review-loop.spec.ts", options: { reviewLoop: true } };

test("no running stack: one is started", () => {
  expect(reusesStack(null, running.file, { reviewLoop: true })).toBe(false);
});

test("the same file with the same options reuses its stack", () => {
  expect(reusesStack(running, running.file, { reviewLoop: true })).toBe(true);
});

test("another file gets a stack of its own, whatever its options", () => {
  expect(reusesStack(running, "/e2e/tests/slice.spec.ts", {})).toBe(false);
  expect(reusesStack(running, "/e2e/tests/slice.spec.ts", { reviewLoop: true })).toBe(false);
});

test("options left out are the defaults", () => {
  const plain = { file: "/e2e/tests/slice.spec.ts", options: {} };
  expect(reusesStack(plain, plain.file, { reviewLoop: false })).toBe(true);
});

test("a test asking for other options than its file's running stack fails with the reason", () => {
  expect(() => reusesStack(running, running.file, { reviewLoop: false })).toThrow(
    /review-loop\.spec\.ts: its tests share one stack, started with reviewLoop: true; this test asks for reviewLoop: false\. Set test\.use\(\{ reviewLoop \}\) for the whole file\./,
  );
});
