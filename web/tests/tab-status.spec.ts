import { expect, test } from "@playwright/test";
import { ANALYSIS_ID } from "./support";

// The tab title says whether work that lasts over 10 s is still running or done (owner request, 2026-11-18):
// "smart-review <short name>#<number>: busy | ready | attention".
// The fixture repository has the short name "NC" in the dashboard settings.
const ID = "smart-review NC#482";
test("the tab title follows a requested flow analysis and an Ask answer", async ({ page }) => {
  await page.clock.install();
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=tab-status#f3`);
  const main = page.getByRole("main");
  await expect(page).toHaveTitle(ID);
  await main.getByRole("button", { name: "Analyse this flow" }).click();
  await page.clock.fastForward(1_100);
  await expect(page.getByText(/Being analysed · started 0 min ago/)).toBeVisible();
  // Under 10 s the title stays as it is.
  await expect(page).toHaveTitle(ID);
  await page.clock.fastForward(11_000);
  await expect(page).toHaveTitle(`${ID}: busy`);
  await page.clock.fastForward(60_000);
  await expect(page).toHaveTitle(`${ID}: ready`);
  // Watched the whole time: the outcome lingers a few seconds only.
  await page.clock.fastForward(5_000);
  await expect(page).toHaveTitle(ID);

  // An Ask answer that takes 15 s.
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Flow 1:/ })
    .click();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Step 5:/ })
    .click();
  await page.keyboard.press("a");
  await page.getByRole("textbox", { name: /^Ask the LLM about / }).fill("Why retry here?");
  await page.keyboard.press("Enter");
  await page.clock.fastForward(1_000);
  await expect(page.getByText("Asked by you: Why retry here?")).toBeVisible();
  await page.clock.fastForward(11_000);
  await expect(page).toHaveTitle(`${ID}: busy`);
  await page.clock.fastForward(5_000);
  await expect(page).toHaveTitle(`${ID}: ready`);
});
