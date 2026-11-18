// Journey (a), part 2: the usage limit pauses the analysis with its resume time and it resumes on its own; a
// failed flow is named in the Tour and its Retry runs it again.
import {
  analysisState,
  analyze,
  checkScreen,
  clock,
  expectRetriedOnlyAfter,
  heading,
  navTo,
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
  // The plan and the PR context were accepted: the review is open while the remaining flows wait (brief §4.2).
  await expect(page.getByRole("link", { name: "Resume review" })).toHaveCount(1);
  await checkScreen(page);

  // Flow 3 fails once the pause is over; the analysis goes on with the other flows and the failed flow stays
  // explicitly failed until it is requested again (ADR-011, lazy flow analysis).
  let failed = { position: 0, title: "" };
  await expect
    .poll(
      async () => {
        const tour = (await (await page.request.get(`/api/v1/analyses/${id}/tour`)).json()) as {
          flows: { position: number; llm: { title: string }; analysis_status?: { state: string } }[];
        };
        const flows = tour.flows.filter((flow) => flow.analysis_status?.state === "failed");
        if (flows.length === 1) failed = { position: flows[0].position, title: flows[0].llm.title };
        return [flows.length, tour.flows.filter((flow) => flow.analysis_status?.state === "analysed").length];
      },
      { timeout: 90_000 },
    )
    .toEqual([1, 4]);
  expect((await analysisState(page, id)).state, "a failed flow does not fail the analysis").toBe("running");
  // The paused job waited for the reset time it was given, then ran once.
  expectRetriedOnlyAfter(stack, Date.parse(resumesAt), "analyse_flow");

  await page.reload();
  await page.getByRole("link", { name: "Resume review" }).click();
  await navTo(page, new RegExp(`^Flow ${failed.position}:`));
  await expect(heading(page)).toContainText(failed.title);
  await expect(page.getByRole("main").getByRole("status")).toHaveText("Failed — Retry");
  await expect(page.getByRole("main")).toContainText("The flow analysis failed. Retry this flow.");
  await checkScreen(page);

  await page.getByRole("button", { name: "Retry this flow" }).click();
  await waitForState(page, id, "finished");
  await page.goto(`/analyses/${id}/progress`);
  await expect(page.getByText("5 of 5 flows analysed")).toBeVisible();
  await expect(page.getByRole("link", { name: "Resume review" })).toBeVisible();
  await checkScreen(page);
});
