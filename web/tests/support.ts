import { expect, type Page } from "@playwright/test";

/** The fixture analysis of fixtures/pr-482. */
export const ANALYSIS_ID = "6d3a2f1e-0482-4c5e-9a7b-000000000482";

/** Opens the review of the fixture analysis in a fixture state and waits for the first block. */
export async function openTour(page: Page, state = "default"): Promise<void> {
  await page.goto(`/analyses/${ANALYSIS_ID}?state=${state}`);
  await expect(page.locator("main h2:not([data-column-title])").first()).toBeVisible();
}

const BANNED = [
  /\bno\s+risks?\b/i,
  /\bnothing\s+to\s+review\b/i,
  /\bno\s+review\s+needed\b/i,
  /\bnoise\b/i,
  /\bverified\b/i,
  /\bwas\s+wrong\b/i,
  /\bconfidence\b/i,
];

/** Asserts the wording rules of brief §1 / plan §12.4 on the page text, tooltips included. */
export async function expectTrustWording(page: Page): Promise<void> {
  const text = await page.locator("body").evaluate((body) => body.textContent ?? "");
  for (const pattern of BANNED) expect(text, `banned wording ${pattern}`).not.toMatch(pattern);
  const withoutPlanUsage = await page.locator("body").evaluate((body) => {
    const copy = body.cloneNode(true) as HTMLElement;
    copy.querySelectorAll("p[data-codex-plan-usage]").forEach((node) => node.remove());
    return copy.textContent ?? "";
  });
  expect(withoutPlanUsage, "percentage outside observed plan usage").not.toMatch(/\d\s?%/);
  expect(await page.getByRole("banner").innerText()).not.toContain("%");
}
