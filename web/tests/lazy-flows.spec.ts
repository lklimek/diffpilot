import { test, expect } from "@playwright/test";
import { ANALYSIS_ID } from "./support";

test("partial review opens pending flows on demand, updates over SSE and keeps background off", async ({
  page,
}) => {
  await page.clock.install();
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=lazy-flows`);
  await expect(page.getByText("1 of 5 flows analysed. Pending flows have not been assessed.")).toBeVisible();
  const background = page.getByRole("checkbox", { name: "Analyse the remaining flows in the background" });
  await background.click();
  await expect(background).not.toBeChecked();
  const requested = page.waitForRequest(
    (r) => r.url().endsWith("/flows/f3/request") && r.method() === "POST",
  );
  await page.getByRole("button", { name: /^Flow 3:/ }).click();
  await requested;
  await expect(page.getByRole("status").filter({ hasText: "Queued" })).toBeVisible();
  await page.clock.fastForward(1_100);
  await expect(page.getByText(/Being analysed · started 0 min ago/)).toBeVisible();
  await page.clock.fastForward(60_000);
  await expect(page.getByText(/Being analysed · started 1 min ago/)).toBeVisible();
  await page.clock.fastForward(11_000);
  await expect(page.getByText("2 of 5 flows analysed. Pending flows have not been assessed.")).toBeVisible();
  await expect(page.getByText(/These changes have not been assessed/)).toHaveCount(0);
  await expect(background).not.toBeChecked();
});
