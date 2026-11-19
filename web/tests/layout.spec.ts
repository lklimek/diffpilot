import { type Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import { ANALYSIS_ID } from "./support";

// Regression: the review page overflowed the window; wheel over the top bar or Space/PageDown scrolled the whole
// layout away. The shell is exactly viewport-high; only the columns scroll.
const open = async (page: Page, scenario: string) => {
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=${scenario}`);
  await expect(page.locator("main h2:not([data-column-title])").first()).toBeVisible();
};
const fits = (page: Page) =>
  page.evaluate(() => ({
    height: document.scrollingElement!.scrollHeight,
    width: document.scrollingElement!.scrollWidth,
    inner: window.innerHeight,
    innerWidth: window.innerWidth,
    scrolled: window.scrollY,
  }));

for (const [width, height] of [
  [1280, 800],
  [1440, 900],
] as const) {
  test(`${width}x${height}: the document never scrolls, on every block of a real tour`, async ({ page }) => {
    await page.setViewportSize({ width, height });
    await open(page, "real-small");
    // Every block in order (Next), then the menu rows (quiet blocks, pending review).
    const next = page.getByRole("button", { name: /^Next/ });
    for (let i = 0; i < 20 && (await next.getAttribute("aria-disabled")) !== "true"; i += 1) {
      const size = await fits(page);
      expect(size.height, `height at block ${i}`).toBe(size.inner);
      expect(size.width).toBe(size.innerWidth);
      const title = await page.locator("main p[aria-live]").innerText();
      await next.click();
      await expect(page.locator("main p[aria-live]")).not.toHaveText(title);
    }
    for (const row of await page.locator("nav[aria-label='Flows and steps'] [data-row]").all()) {
      await row.click();
      const size = await fits(page);
      expect(size.height, `height at row ${await row.innerText()}`).toBe(size.inner);
    }
  });
}

test("Space, PageDown and the wheel over the top bar never scroll the document", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await open(page, "real-small");
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Flow 1:/ })
    .click();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Step 4:/ })
    .click();
  await page.locator("body").press("PageDown");
  await page.keyboard.press("Space");
  await page.getByRole("banner").hover();
  await page.mouse.wheel(0, 2000);
  await page.waitForTimeout(200);
  expect((await fits(page)).scrolled).toBe(0);
  await expect(page.getByRole("banner")).toBeInViewport();
});

test("PageDown scrolls the focused column", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await open(page, "real-small");
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Flow 1:/ })
    .click();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Step 4:/ })
    .click();
  const column = page.locator("[data-column=main] .overflow-auto").first();
  await column.click({ position: { x: 4, y: 4 } });
  await page.keyboard.press("PageDown");
  await expect.poll(() => column.evaluate((el) => el.scrollTop)).toBeGreaterThan(0);
  expect((await fits(page)).scrolled).toBe(0);
});

test("opening a changed step reveals its first changed line and keeps code inside its column", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await open(page, "real-tiny");
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Flow 1:/ })
    .click();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Step 2:/ })
    .click();
  const changed = page.locator("main [data-code-slot] [data-line='43'][data-side=head]");
  await expect(changed).toBeInViewport();
  const code = page.getByRole("main").getByRole("region", { name: /lines 28 to 47/ });
  if (await code.evaluate((el) => el.scrollWidth > el.clientWidth)) {
    await code.evaluate((el) => {
      el.scrollLeft = 500;
    });
    expect(await code.evaluate((el) => el.scrollLeft)).toBeGreaterThan(0);
  }
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const width = await page.locator("[data-column=main]").evaluate((el) => el.clientWidth);
  // Default side columns follow the mockup proportions, so the code column keeps about 700 px at 1280.
  expect(width).toBeGreaterThanOrEqual(690);
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Step 3:/ })
    .click();
  expect(await page.locator("[data-column=main]").evaluate((el) => el.clientWidth)).toBe(width);
});

test("resizing persists across steps and reload; hiding inspector gives code space", async ({ page }) => {
  await open(page, "real-small");
  const separator = page.getByRole("separator", { name: /Resize the Inspector/ });
  await separator.focus();
  await page.keyboard.press("ArrowLeft");
  const width = () => page.locator("[data-column=inspector]").evaluate((el) => el.clientWidth);
  const resized = await width();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Flow 1:/ })
    .click();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Step 2:/ })
    .click();
  expect(await width()).toBe(resized);
  await page.reload();
  await expect(page.getByRole("main")).toBeVisible();
  await expect.poll(width).toBe(resized);
  const codeWidth = await page.locator("[data-column=main]").evaluate((el) => el.clientWidth);
  await page.getByRole("button", { name: "Hide Inspector column" }).click();
  expect(await page.locator("[data-column=main]").evaluate((el) => el.clientWidth)).toBeGreaterThan(
    codeWidth,
  );
});

for (const width of [1280, 1440]) {
  test(`real tour light screenshots at ${width}`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: "light" });
    await page.setViewportSize({ width, height: width === 1280 ? 800 : 900 });
    await open(page, "real-tiny");
    await page
      .getByRole("navigation")
      .getByRole("button", { name: /^Flow 1:/ })
      .click();
    await page
      .getByRole("navigation")
      .getByRole("button", { name: /^Step 2:/ })
      .click();
    await expect(page.locator("main [data-code-slot] [data-line='43'][data-side=head]")).toBeInViewport();
    await page.screenshot({ path: `/data/artifacts/smart-review/2026-11-17/wp3-tour-10/step-${width}.png` });
    await page
      .getByRole("navigation")
      .getByRole("button", { name: /Docs & comments/ })
      .click();
    await page.getByRole("button", { name: "Show lines 24–27" }).click();
    await expect(page.getByRole("button", { name: "Comment on line 24" })).toBeVisible();
    await page.screenshot({ path: `/data/artifacts/smart-review/2026-11-17/wp3-tour-10/docs-${width}.png` });
  });
}

test("deep links survive reload and browser history, and evidence opens the cited step", async ({ page }) => {
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=real-tiny#f1.2`);
  await expect(page.locator("main [data-code-slot] [data-line='43']")).toBeInViewport();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Step 3:/ })
    .click();
  await expect(page).toHaveURL(/#f1\.3$/);
  await page.goBack();
  await expect(page.locator("main [data-code-slot] [data-line='43']")).toBeInViewport();
  await page.goForward();
  await expect(page).toHaveURL(/#f1\.3$/);
  await page.reload();
  await expect(page.getByRole("navigation").getByRole("button", { name: /^Step 3:/ })).toHaveAttribute(
    "aria-current",
    "step",
  );
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^PR context/ })
    .click();
  await page
    .getByRole("main")
    .getByText(/Evidence ·/)
    .first()
    .click();
  await page
    .getByRole("main")
    .getByRole("button", { name: "Show src/ui/components/subscreen_chooser_panel.rs:43" })
    .click();
  await expect(page).toHaveURL(/#f1\.2\?file=/);
  await expect(page.locator("main [data-code-slot] [data-line='43']")).toBeInViewport();
});

test("Ask evidence opens code on its cited step or in the standalone range", async ({ page }) => {
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=real-small#f1.5`);
  const answers = page.getByRole("list", { name: "Ask conversation" });
  await answers.getByText(/Evidence ·/).click();
  await answers.getByRole("button", { name: "Show internal/statesync/reactor.go:1171" }).click();
  await expect(page).toHaveURL(/#f1\.3\?file=/);
  await expect(page.locator("main [data-code-slot] [data-line='1171']")).toBeInViewport();
  await page.goBack();
  await answers.getByText(/Evidence ·/).click();
  await answers.getByRole("button", { name: "Show internal/statesync/reactor.go:1286" }).click();
  await expect(page).toHaveURL(/#lines\?file=/);
  await expect(page.getByRole("button", { name: "Comment on line 1286" })).toBeVisible();
});

test("long Ask prose and fenced code wrap inside the inspector", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`/analyses/${ANALYSIS_ID}?scenario=long-answer#f1.5`);
  const answers = page.getByRole("list", { name: "Ask conversation" });
  const prose = answers.getByText("longword".repeat(100));
  await expect(prose).toBeVisible();
  await expect(prose).toHaveCSS("overflow-wrap", "anywhere");
  const code = answers.locator("pre");
  await expect(code).toHaveCSS("white-space", "pre-wrap");
  for (const element of [answers, prose, code]) {
    const size = await element.evaluate((node) => ({ width: node.clientWidth, scroll: node.scrollWidth }));
    expect(size.scroll).toBeLessThanOrEqual(size.width + 1);
  }
});
