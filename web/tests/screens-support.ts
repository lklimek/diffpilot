import AxeBuilder from "@axe-core/playwright";
import { expect, type Page } from "@playwright/test";

/** The fixture PR of fixtures/pr-482. */
export const PR_URL = "https://github.com/acme/node-client/pull/482";

/** Fills the PR URL and presses Check (a lookup; nothing starts). */
export async function analyze(page: Page, url = PR_URL): Promise<void> {
  await page.getByLabel("GitHub pull request URL").fill(url);
  await page.getByRole("button", { name: "Check" }).click();
}

/** Starts the analysis from the check card (`name`: "Start analysis" or "Start analysis anyway"). */
export async function startAnalysis(page: Page, name = "Start analysis"): Promise<void> {
  await page
    .getByRole("region", { name: "Pull request check" })
    .getByRole("button", { name, exact: true })
    .click();
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

/**
 * Screen-wide: every rendered tooltip trigger can take keyboard focus (itself or a focusable element inside it)
 * and its tip shows on focus, as the full-stack `sweepTooltips` checks; no allow-list.
 */
export async function expectKeyboardTooltips(page: Page): Promise<void> {
  const problems = await page.evaluate(async () => {
    const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
    const found: string[] = [];
    const focusable = "button, a[href], input, select, textarea, [tabindex]:not([tabindex='-1'])";
    for (const trigger of document.querySelectorAll<HTMLElement>("[aria-describedby]")) {
      const tip = (trigger.getAttribute("aria-describedby") ?? "")
        .split(/\s+/)
        .map((id) => document.getElementById(id))
        .find((element) => element?.getAttribute("role") === "tooltip");
      if (!tip || !trigger.checkVisibility({ contentVisibilityAuto: true, visibilityProperty: true }))
        continue;
      const name = (trigger.getAttribute("aria-label") ?? trigger.textContent ?? "").trim().slice(0, 60);
      const target = trigger.matches(focusable) ? trigger : trigger.querySelector<HTMLElement>(focusable);
      target?.focus({ preventScroll: true });
      await frame();
      await frame();
      if (!target || document.activeElement !== target) found.push(`cannot take keyboard focus: "${name}"`);
      else if (tip.hidden) found.push(`tooltip not shown on focus: "${name}"`);
      target?.blur();
    }
    return found;
  });
  expect(problems, "tooltip triggers").toEqual([]);
}
