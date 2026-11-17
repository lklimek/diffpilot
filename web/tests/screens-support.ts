import AxeBuilder from "@axe-core/playwright";
import { expect, type Page } from "@playwright/test";

/** The fixture PR of fixtures/pr-482. */
export const PR_URL = "https://github.com/acme/node-client/pull/482";

/** Fills the PR URL and presses Analyze. */
export async function analyze(page: Page, url = PR_URL): Promise<void> {
  await page.getByLabel("GitHub pull request URL").fill(url);
  await page.getByRole("button", { name: "Analyze" }).click();
}

/** Every button and button-like link in main has a tooltip (brief §4 rule 3). */
export async function expectTooltips(page: Page): Promise<void> {
  const main = page.getByRole("main");
  for (const control of [
    ...(await main.getByRole("button").all()),
    ...(await main.locator("a[class*='rounded-md']").all()),
  ]) {
    const name = await control.innerText();
    expect(await control.getAttribute("aria-describedby"), `tooltip of "${name}"`).toBeTruthy();
  }
}

/** No serious or critical axe violation (WCAG 2.2 AA tags). */
export async function expectAccessible(page: Page): Promise<void> {
  const results = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"])
    .analyze();
  const serious = results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
  expect(serious.map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(" ")}`)).toEqual([]);
}
