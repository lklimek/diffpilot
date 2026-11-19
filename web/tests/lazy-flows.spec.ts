import { expect, test } from "./fixtures";
import { ANALYSIS_ID } from "./support";

test("flows run on request only: opening one requests nothing, Analyse this flow runs it over SSE", async ({
  page,
}) => {
  let requests = 0;
  page.on("request", (r) => {
    if (r.method() === "POST" && /\/flows\/[^/]+\/request$/.test(r.url())) requests += 1;
  });
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=lazy-flows`);
  const work = page.getByRole("region", { name: "Analysis work" });
  await expect(work.getByText("1 of 5 flows analysed. Pending flows have not been assessed.")).toBeVisible();
  await expect(work.getByRole("status")).toHaveText(
    "Nothing is being analysed. Choose a flow to analyse, or analyse all remaining flows.",
  );
  await expect(work.getByRole("button", { name: "Analyse all remaining flows" })).toBeVisible();
  await page.getByRole("button", { name: /^Flow 3:/ }).click();
  await expect(page.getByRole("main").getByText("Not analysed yet")).toBeVisible();
  expect(requests).toBe(0);
  const requested = page.waitForRequest(
    (r) => r.url().endsWith("/flows/f3/request") && r.method() === "POST",
  );
  await page.getByRole("main").getByRole("button", { name: "Analyse this flow" }).click();
  await requested;
  await expect(page.getByRole("status").filter({ hasText: "Queued" })).toBeVisible();
  await page.clock.fastForward(1_100);
  await expect(page.getByText(/Being analysed · started 0 min ago/)).toBeVisible();
  await page.clock.fastForward(60_000);
  await expect(page.getByText(/Being analysed · started 1 min ago/)).toBeVisible();
  await page.clock.fastForward(11_000);
  await expect(work.getByText("2 of 5 flows analysed. Pending flows have not been assessed.")).toBeVisible();
  await expect(page.getByText(/These changes have not been assessed/)).toHaveCount(0);
});

test("Analyse all remaining flows switches background analysis on and off", async ({ page }) => {
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=lazy-flows`);
  const work = page.getByRole("region", { name: "Analysis work" });
  const put = page.waitForRequest((r) => r.url().endsWith("/background") && r.method() === "PUT");
  await work.getByRole("button", { name: "Analyse all remaining flows" }).click();
  expect((await put).postDataJSON()).toEqual({ enabled: true });
  // Like the server, the switch queues the next flow by rank: the panel no longer says nothing is analysed.
  await expect(work.getByRole("status")).toHaveCount(0);
  await expect(
    page
      .getByRole("navigation")
      .getByText(/^(Queued|Being analysed)/)
      .first(),
  ).toBeVisible();
  await work.getByRole("button", { name: "Analysing all remaining flows — Stop" }).click();
  await expect(work.getByRole("button", { name: "Analyse all remaining flows" })).toBeVisible();
});
