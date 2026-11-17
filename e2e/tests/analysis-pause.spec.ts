// Journey (a), part 2: the usage limit pauses the analysis with its resume time and it resumes on its own; a
// failed flow is named and Retry starts a new round.
import {
  analysisState,
  analyze,
  checkScreen,
  clock,
  expectRetriedOnlyAfter,
  signIn,
  waitForState,
} from "../support/app";
import { expect, test } from "../support/fixtures";

test("usage limit → paused until a time → resumed; a failed flow is named → Retry → finished", async ({
  page,
  stack,
}) => {
  test.skip(!stack.script, "needs the fake provider's script (a local stack)");
  stack.script?.([
    { id: "flow-3-fails", job: "analyse_flow", flow: 3, times: 1, action: { fail: { message: "scripted" } } },
    {
      id: "pause",
      job: "analyse_flow",
      flow: 2,
      times: 1,
      action: { usage_limit: { resets_in_secs: 12 } },
    },
  ]);
  await signIn(page);
  const id = await analyze(page);
  // Flows run in order: flow 2 hits the usage limit, the analysis resumes on its own, then flow 3 fails.
  await waitForState(page, id, "paused");
  const paused = await analysisState(page, id);
  const resumesAt = String(paused.resumes_at);
  expect(Date.parse(resumesAt), "the resume time lies ahead").toBeGreaterThan(Date.now());
  await expect(
    page.getByText(`Paused until ${await clock(page, resumesAt)} — ${String(paused.done)} of 5`),
  ).toBeVisible();
  await expect(page.getByRole("link", { name: "Open review" })).toHaveCount(0);
  await checkScreen(page);

  await waitForState(page, id, "failed");
  await expect(page.getByText(/Analysis of flow “.+” failed/)).toBeVisible();
  await checkScreen(page);
  // The paused job waited for the reset time it was given, then ran once.
  expectRetriedOnlyAfter(stack, Date.parse(resumesAt), "analyse_flow");

  await page.getByRole("button", { name: "Retry" }).click();
  await waitForState(page, id, "finished");
  await expect(page.getByText("5 of 5 flows analysed")).toBeVisible();
  await expect(page.getByRole("link", { name: "Open review" })).toBeVisible();
  await checkScreen(page);
});
