// Journey (d): feedback values on a step, stored here and written to fake MemCan (pending → written), removal
// deletes the memory, a write MemCan fails stays recorded here and is retried once MemCan answers again (hard
// case 1: a confident wrong HIGH).
import { type Page } from "@playwright/test";
import { checkScreen, finishedReview, inspector, memcan, openStep } from "../support/app";
import { expect, test } from "../support/fixtures";

test.describe.configure({ mode: "serial" });

const buttons = (page: Page) => inspector(page).getByRole("group", { name: /^Feedback on / });
// The writer is nudged by every feedback change, so an outcome shows within seconds.
const OUTCOME_MS = 10_000;
// A failed entry is retried 30 s later; the writer's next unnudged sweep comes at most 60 s after the last one
// (crates/server/src/memcan/writer.rs FIRST_RETRY, SWEEP_INTERVAL).
const RETRY_MS = 75_000;
// The open page re-reads a failed entry every 15 s (web/src/api/queries.ts FEEDBACK_RETRY_POLL_MS).
const SHOWN_MS = RETRY_MS + 15_000;

const memory = (page: Page) => inspector(page).getByRole("list", { name: "Calibration memory" });

interface Feedback {
  id: string;
  value: string;
  memory?: string;
}

async function stored(page: Page, id: string): Promise<Feedback[]> {
  return (await (await page.request.get(`/api/v1/analyses/${id}/feedback`)).json()) as Feedback[];
}

test("hard case 1: wrong + overrated on the HIGH step are stored; MemCan gets a calibration example", async ({
  page,
  stack,
}) => {
  test.skip(!stack.memcan, "needs the fake MemCan's inspection endpoint");
  const fake = memcan(page, stack);
  const id = await finishedReview(page);
  await openStep(page, 1, 5);
  await expect(inspector(page)).toContainText("HIGH");
  // A slow MemCan keeps the write pending long enough to be seen.
  await fake.mode({ mode: "delay", ms: 3000 });
  await buttons(page).getByRole("button", { name: "wrong" }).click();
  await expect(inspector(page).getByRole("status")).toHaveText("Feedback saved: wrong.");
  await buttons(page).getByRole("button", { name: "overrated" }).click();
  await expect(memory(page)).toContainText("overrated: Recorded; saving to calibration memory…");
  await checkScreen(page);
  await expect(memory(page)).toContainText("overrated: Saved to calibration memory", { timeout: 30_000 });
  // Only overrated / underrated are calibration examples (brief §3); wrong is stored in MemCan as plain feedback.
  await expect(memory(page)).toContainText(/wrong: Recorded(?!;| here)/, { timeout: 30_000 });
  expect((await stored(page, id)).find((entry) => entry.value === "wrong")?.memory).toBe("written");
  await expect(buttons(page).getByRole("button", { name: "overrated" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await fake.mode({ mode: "normal" });

  const overrated = (await stored(page, id)).find((entry) => entry.value === "overrated");
  expect(overrated).toBeDefined();
  const written = (await fake.state()).memories[overrated?.id ?? ""];
  expect(written).toMatchObject({
    type: "severity-calibration",
    source: "smart-review",
    value: "overrated",
    project: "node-client",
    user_id: "project:node-client",
    repo: "acme/node-client",
    pr: 482,
  });
  expect(String(written.data)).toContain("too high (overrated)");
  await checkScreen(page);
});

test("pressing a stored value again removes it and deletes its memory", async ({ page, stack }) => {
  test.skip(!stack.memcan, "needs the fake MemCan's inspection endpoint");
  const fake = memcan(page, stack);
  const id = await finishedReview(page);
  await openStep(page, 1, 5);
  const overrated = (await stored(page, id)).find((entry) => entry.value === "overrated");
  expect(overrated, "stored by the previous test").toBeDefined();
  await buttons(page).getByRole("button", { name: "overrated" }).click();
  await expect(inspector(page).getByRole("status")).toHaveText("Feedback removed: overrated.");
  await expect(buttons(page).getByRole("button", { name: "overrated" })).toHaveAttribute(
    "aria-pressed",
    "false",
  );
  await expect
    .poll(async () => (await fake.state()).deleted, { timeout: OUTCOME_MS })
    .toContain(overrated?.id);
  expect((await stored(page, id)).map((entry) => entry.value)).not.toContain("overrated");
  await checkScreen(page);
});

test("MemCan cannot embed the record (MemCan's failure): recorded here, not reported as saved", async ({
  page,
  stack,
}) => {
  test.skip(!stack.memcan, "needs the fake MemCan's inspection endpoint");
  const fake = memcan(page, stack);
  await finishedReview(page);
  await openStep(page, 1, 7);
  await fake.mode({ mode: "embedding_failure" });
  await buttons(page).getByRole("button", { name: "underrated" }).click();
  await expect(memory(page)).toContainText(
    "underrated: Recorded here; not yet saved to calibration memory (retrying)",
    { timeout: OUTCOME_MS },
  );
  await checkScreen(page);
  const state = await fake.state();
  expect(
    state.calls.some((call) => call.tool === "_import_records"),
    "the write reached MemCan",
  ).toBe(true);
  expect(Object.values(state.memories).map((entry) => entry.value)).not.toContain("underrated");
  await fake.mode({ mode: "normal" });
});

test("MemCan rejects the call (synthetic tool error): recorded here, not reported as saved", async ({
  page,
  stack,
}) => {
  test.skip(!stack.memcan, "needs the fake MemCan's inspection endpoint");
  const fake = memcan(page, stack);
  await finishedReview(page);
  await openStep(page, 1, 4);
  await fake.mode({ mode: "synthetic_tool_error" });
  await buttons(page).getByRole("button", { name: "irrelevant" }).click();
  await expect(memory(page)).toContainText("irrelevant: Recorded here; not yet saved to memory (retrying)", {
    timeout: OUTCOME_MS,
  });
  await checkScreen(page);
  expect(Object.values((await fake.state()).memories).map((entry) => entry.value)).not.toContain(
    "irrelevant",
  );
});

test("once MemCan answers again, the rejected write is retried and reads saved", async ({ page, stack }) => {
  test.skip(!stack.memcan, "needs the fake MemCan's inspection endpoint");
  const fake = memcan(page, stack);
  const id = await finishedReview(page);
  await openStep(page, 1, 4);
  await expect(memory(page)).toContainText("irrelevant: Recorded here; not yet saved", {
    timeout: OUTCOME_MS,
  });
  await fake.mode({ mode: "normal" });
  const irrelevant = (await stored(page, id)).find((entry) => entry.value === "irrelevant");
  await expect
    .poll(async () => (await stored(page, id)).find((entry) => entry.id === irrelevant?.id)?.memory, {
      timeout: RETRY_MS,
    })
    .toBe("written");
  expect((await fake.state()).memories[irrelevant?.id ?? ""]).toMatchObject({ value: "irrelevant" });
  await page.reload();
  await expect(memory(page)).toContainText(/irrelevant: Recorded(?!;| here)/);
  await checkScreen(page);
});

test("the open page shows a retried write as saved without a reload", async ({ page, stack }) => {
  // Analysis (≤ 90 s) plus the retry as the page shows it (≤ 90 s).
  test.setTimeout(240_000);
  const fake = memcan(page, stack);
  await finishedReview(page);
  await openStep(page, 1, 3);
  await fake.mode({ mode: "synthetic_tool_error" });
  await buttons(page).getByRole("button", { name: "correct" }).click();
  await expect(memory(page)).toContainText("correct: Recorded here; not yet saved", { timeout: OUTCOME_MS });
  await fake.mode({ mode: "normal" });
  await expect(memory(page)).toContainText(/correct: Recorded(?!;| here)/, { timeout: SHOWN_MS });
  await checkScreen(page);
});
