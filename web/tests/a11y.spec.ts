import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";
import { openTour } from "./support";

test("landmarks: banner, navigation, main", async ({ page }) => {
  await openTour(page);
  await expect(page.getByRole("banner")).toHaveCount(1);
  await expect(page.getByRole("navigation", { name: "Flows and steps" })).toBeVisible();
  await expect(page.getByRole("main")).toHaveCount(1);
});

test("no serious axe violations on the three blocks", async ({ page }) => {
  await openTour(page);
  for (const go of [
    async () => {},
    () =>
      page
        .getByRole("navigation")
        .getByRole("button", { name: "Flow 1: Send an RPC request with retry" })
        .click(),
    () => page.getByRole("navigation").getByRole("button", { name: "Step 5: Retry allowed?" }).click(),
  ]) {
    await go();
    const results = await new AxeBuilder({ page })
      .withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"])
      .analyze();
    const serious = results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
    expect(serious.map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(" ")}`)).toEqual([]);
  }
});

test("keyboard: skip link, menu and Next reachable by Tab with a visible focus ring", async ({ page }) => {
  await openTour(page);
  await page.keyboard.press("Tab");
  const skip = page.getByRole("link", { name: "Skip to content" });
  await expect(skip).toBeFocused();
  await expect(skip).toBeInViewport();
  const reached: string[] = [];
  for (let i = 0; i < 60; i += 1) {
    await page.keyboard.press("Tab");
    const name = await page.evaluate(() => {
      const el = document.activeElement as HTMLElement | null;
      return el ? (el.getAttribute("aria-label") ?? el.textContent ?? "").trim() : "";
    });
    reached.push(name);
    if (name.startsWith("Next:")) break;
  }
  expect(reached).toContain("PR context");
  expect(reached).toContain("Flow 1: Send an RPC request with retry");
  expect(reached.at(-1)).toMatch(/^Next: Flow 1/);
  const outline = await page.evaluate(() => getComputedStyle(document.activeElement!).outlineStyle);
  expect(outline).toBe("solid");
  await page.keyboard.press("Enter");
  await expect(page.getByRole("main").getByRole("heading", { level: 2 }).first()).toHaveText(
    "1 Send an RPC request with retry",
  );
});
