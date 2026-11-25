import AxeBuilder from "@axe-core/playwright";
import { type Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import { ANALYSIS_ID, expectTrustWording } from "./support";

// The Files view (brief §4.5) against the msw mocks of fixtures/pr-482, at a viewport wide enough for the side
// panels (≥ 1600 px).
const FILES = `/analyses/${ANALYSIS_ID}/files`;
const link = (file: string, extra = "") =>
  `${FILES}#lines?file=${encodeURIComponent(file)}&side=head${extra}`;

const inspector = (page: Page) => page.getByRole("complementary", { name: "Inspector" });
const inspectorTitle = (page: Page) => inspector(page).locator("h2:not([data-column-title])").first();
const fileTitle = (page: Page) => page.getByRole("main").locator("h2:not([data-column-title])").first();

async function openFiles(page: Page, url = FILES) {
  await page.setViewportSize({ width: 1600, height: 900 });
  await page.goto(url);
  await expect(fileTitle(page)).toBeVisible();
}

test("opens the first file, folds unchanged lines, and Show loads them by range", async ({ page }) => {
  await openFiles(page, link("src/rpc/client.rs"));
  await expect(page.getByRole("navigation", { name: "Files" })).toBeVisible();
  await expect(fileTitle(page)).toHaveText("src/rpc/client.rs");
  const fold = page.locator("[data-unchanged-fold]").first();
  await expect(fold).toContainText("31 unchanged lines (1–31)");
  await fold.getByRole("button", { name: /^Show 31 unchanged lines/ }).click();
  await expect(page.locator('[data-line="1"][data-side="head"]')).toBeVisible();
  await expectTrustWording(page);
});

test("one ◆ per LLM remark in the gutter, and none on lines without one", async ({ page }) => {
  await openFiles(page, link("src/rpc/client.rs"));
  const code = page.getByRole("main");
  await expect(code.locator('[data-line="99"] [data-mark=remark]')).toHaveCount(3);
  await expect(code.locator('[data-line="97"] [data-mark=remark]')).toHaveCount(1);
  await expect(code.locator('[data-line="95"] [data-mark=remark]')).toHaveCount(0);
  // Hover shows the popup, leaving hides it; a click pins it until the next click.
  const marker = code.locator('[data-line="97"] [data-mark=remark]');
  await marker.hover();
  await expect(page.getByRole("dialog", { name: /^LLM remark at line 97/ })).toBeVisible();
  await page.mouse.move(5, 5);
  await expect(page.getByRole("dialog", { name: /^LLM remark at line 97/ })).toBeHidden();
  await marker.click();
  await page.mouse.move(5, 5);
  const pinned = page.getByRole("dialog", { name: /^LLM remark at line 97/ });
  await expect(pinned).toBeVisible();
  await pinned.getByRole("button", { name: /^Show in Inspector/ }).click();
  await expect(inspectorTitle(page)).toHaveText(/^Step 3 of flow 1 · /);
  await expect(inspector(page).getByRole("button", { name: "← Back to hunk" })).toBeVisible();
});

test("a hunk click lists its flows and steps; Details narrows; Esc goes back", async ({ page }) => {
  await openFiles(page, link("src/rpc/client.rs"));
  await page.locator('[data-line="104"] code').click();
  await expect(inspectorTitle(page)).toHaveText("client.rs · hunk, lines 96–109 (+ 2 removed)");
  await expect(page).toHaveURL(/line=96&end=109$/);
  const flow = inspector(page).getByRole("group", { name: /^Flow 1/ });
  await expect(flow.getByRole("listitem")).toHaveCount(7);
  await expect(flow).toContainText("☐ not ticked in Flows");
  await expect(inspector(page).getByRole("button", { name: /Mark reviewed/ })).toHaveCount(0);
  await flow.getByRole("button", { name: /^Details: Step 6/ }).click();
  await expect(inspectorTitle(page)).toHaveText(/^Step 6 of flow 1 · /);
  await page.keyboard.press("Escape");
  await expect(inspectorTitle(page)).toHaveText(/^client\.rs · hunk/);
  await page.keyboard.press("Escape");
  await expect(inspectorTitle(page)).toHaveText("src/rpc/client.rs");
});

test("line numbers select lines (click, Shift+click, drag); the permalink reopens them; Ask is anchored", async ({
  page,
}) => {
  await openFiles(page, link("src/rpc/client.rs"));
  const number = (n: number) => page.getByRole("button", { name: `Select line ${n}`, exact: true });
  await number(100).click();
  await number(102).click({ modifiers: ["Shift"] });
  await expect(page).toHaveURL(/line=100&end=102$/);
  await expect(inspectorTitle(page)).toHaveText("Your selection · client.rs lines 100–102");
  await expect(page.locator("[data-selected-line]")).toHaveCount(3);
  // Drag over line numbers.
  await number(104).hover();
  await page.mouse.down();
  await number(106).hover();
  await page.mouse.up();
  await expect(inspectorTitle(page)).toHaveText("Your selection · client.rs lines 104–106");
  // The permalink opens the same selection after a reload, without a new history entry per click.
  const url = page.url();
  await page.reload();
  await expect(inspectorTitle(page)).toHaveText("Your selection · client.rs lines 104–106");
  expect(page.url()).toBe(url);
  await inspector(page).getByRole("button", { name: "Ask about these lines" }).click();
  const chat = page.getByRole("dialog", { name: "Ask" });
  const field = chat.getByRole("textbox", { name: "Your question" });
  await expect(field).toBeFocused();
  await field.fill("Why return Exhausted here?");
  await field.press("Enter");
  const thread = chat.locator("[data-thread]").last();
  await expect(thread).toContainText("Why return Exhausted here?");
  await expect(
    thread.getByRole("button", {
      name: "Step 1.7 · client.rs 104–106 · src/rpc/client.rs:104–106 (new code)",
    }),
  ).toBeVisible();
  // The Inspector keeps a read-only copy of the selection's threads.
  await expect(
    inspector(page).getByRole("region", { name: /^Your questions about these lines/ }),
  ).toContainText("Why return Exhausted here?");
});

test("v marks Viewed on GitHub, then the next file opens; the tree and Flows show the mark", async ({
  page,
}) => {
  await openFiles(page, link("src/lib.rs"));
  const tree = page.getByRole("navigation", { name: "Files" });
  await expect(tree).toContainText("Viewed on GitHub: 3 / 13 files");
  await page.keyboard.press("v");
  await expect(fileTitle(page)).toHaveText("src/rpc/client.rs");
  await expect(fileTitle(page)).toBeFocused();
  await expect(tree).toContainText("Viewed on GitHub: 4 / 13 files");
  // Flows: the step header of a lib.rs step shows the mark.
  await page.goto(`/analyses/${ANALYSIS_ID}#f5.2`);
  await expect(page.locator("[data-viewed]")).toHaveText("☑ viewed");
});

test("a GitHub refusal is shown plainly and the file stays", async ({ page }) => {
  await openFiles(page, `${link("src/lib.rs")}`.replace("#", "?scenario=viewed-refused#"));
  await page.getByRole("button", { name: "Viewed", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("Not saved on GitHub: GitHub refused the change.");
  await expect(fileTitle(page)).toHaveText("src/lib.rs");
});

test("the breadcrumb switches views and keeps the place; s goes back", async ({ page }) => {
  await openFiles(page, link("src/rpc/client.rs", "&line=99&step=f1.5"));
  const crumbs = page.getByRole("navigation", { name: "Breadcrumb" });
  await expect(crumbs.getByRole("link", { name: "diffpilot" })).toHaveAttribute("href", "/");
  await crumbs.getByRole("button", { name: "View: Files" }).click();
  await crumbs.getByRole("link", { name: "Flows" }).click();
  await expect(page).toHaveURL(new RegExp(`/analyses/${ANALYSIS_ID}#f1\\.5$`));
  await expect(page.getByRole("main").locator("h2:not([data-column-title])").first()).toContainText(
    "Retry allowed?",
  );
  await page.keyboard.press("s");
  await expect(page).toHaveURL(/\/files#lines\?file=src%2Frpc%2Fclient\.rs&side=head&line=99&step=f1\.5$/);
  await expect(inspectorTitle(page)).toHaveText(/^Step 5 of flow 1 · Retry allowed\?/);
});

test("no serious axe violations in the Files view's states", async ({ page }) => {
  test.setTimeout(120_000);
  for (const url of [
    link("src/rpc/client.rs"),
    link("src/rpc/client.rs", "&line=96&end=109"),
    link("src/rpc/client.rs", "&line=99&step=f1.5"),
    link("src/rpc/client.rs", "&line=100&end=102"),
    link("src/rpc/metrics.rs", "&line=40&end=46"),
    link("tests/rpc_retry.rs"),
    link("Cargo.lock"),
  ]) {
    for (const colorScheme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme });
      await openFiles(page, url);
      await page.waitForLoadState("networkidle");
      const results = await new AxeBuilder({ page })
        .withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"])
        .analyze();
      // Line numbers keep the code's line height (WCAG 2.5.8 exception, as in the Flows view).
      const lineNumber = (v: { id: string }, html: string) =>
        v.id === "target-size" && html.includes("data-line-action");
      const serious = results.violations
        .filter((v) => v.impact === "serious" || v.impact === "critical")
        .map((v) => ({ ...v, nodes: v.nodes.filter((n) => !lineNumber(v, n.html)) }))
        .filter((v) => v.nodes.length > 0);
      expect(
        serious.map((v) => `${url} ${colorScheme} ${v.id}: ${v.nodes.map((n) => n.target).join(" ")}`),
      ).toEqual([]);
    }
  }
});

test("the document never scrolls; the code stays inside its column", async ({ page }) => {
  for (const [width, height] of [
    [1600, 900],
    [2560, 1080],
  ] as const) {
    await page.setViewportSize({ width, height });
    await page.goto(link("src/rpc/client.rs", "&line=96&end=109"));
    await expect(fileTitle(page)).toBeVisible();
    const sizes = await page.evaluate(() => ({
      scroll: document.documentElement.scrollHeight,
      client: document.documentElement.clientHeight,
      width: document.documentElement.scrollWidth,
      clientWidth: document.documentElement.clientWidth,
    }));
    expect(sizes.scroll).toBeLessThanOrEqual(sizes.client);
    expect(sizes.width).toBeLessThanOrEqual(sizes.clientWidth);
    const main = await page.locator("[data-column=main]").boundingBox();
    const code = await page.locator("[data-column=main] [role=region]").first().boundingBox();
    expect(code!.x + code!.width).toBeLessThanOrEqual(main!.x + main!.width + 1);
  }
});

test("the flows dropdown and the Why this flow strip are keyboard reachable and accessible", async ({
  page,
}) => {
  await openFiles(page, link("src/rpc/client.rs"));
  const progress = page.getByRole("button", { name: /^Your progress 0 \/ 5/ });
  await progress.click();
  const flows = page.getByRole("region", { name: "Flows: analysis and review" });
  await expect(flows.getByRole("listitem")).toHaveCount(5);
  const results = await new AxeBuilder({ page })
    .include("header")
    .withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"])
    .analyze();
  expect(
    results.violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id),
  ).toEqual([]);
  await page.keyboard.press("Escape");
  await expect(flows).toBeHidden();
  await expect(progress).toBeFocused();
  await page.goto(`/analyses/${ANALYSIS_ID}#f1.5`);
  const strip = page.getByRole("region", { name: "Why this flow" });
  await expect(strip).toContainText("Problem:");
  await strip.getByRole("button", { name: /^Why this flow/ }).click();
  await page.reload();
  await expect(
    page.getByRole("region", { name: "Why this flow" }).getByRole("button", { name: /^Why this flow/ }),
  ).toHaveAttribute("aria-expanded", "false");
});

// Screenshots for the owner's comparison with the mockup; only with SCREENSHOT_DIR set.
const dir = process.env.SCREENSHOT_DIR;
test("screenshots light and dark at 1600×900 and 2560×1080", async ({ page }) => {
  test.skip(!dir, "SCREENSHOT_DIR not set");
  const states: [string, string][] = [
    ["file", link("src/rpc/client.rs")],
    ["hunk", link("src/rpc/client.rs", "&line=96&end=109")],
    ["step", link("src/rpc/client.rs", "&line=99&step=f1.5")],
    ["lines", link("src/rpc/client.rs", "&line=100&end=102")],
    ["unexplained", link("src/rpc/metrics.rs", "&line=40&end=46")],
    ["tests", link("tests/rpc_retry.rs")],
    ["one-step-hunk", link("src/config.rs", "&line=23")],
    ["removed-remarks", link("src/rpc/client.rs", "&line=42")],
    ["removed-selection", `${FILES}#lines?file=src%2Frpc%2Fclient.rs&side=base&line=52&end=55`],
  ];
  for (const [width, height] of [
    [1600, 900],
    [2560, 1080],
  ] as const) {
    for (const colorScheme of ["light", "dark"] as const) {
      await page.setViewportSize({ width, height });
      await page.emulateMedia({ colorScheme });
      for (const [name, url] of states) {
        await page.goto(url);
        await expect(fileTitle(page)).toBeVisible();
        await page.waitForLoadState("networkidle");
        await page.screenshot({ path: `${dir}/files-${name}-${colorScheme}-${width}x${height}.png` });
      }
      await page.getByRole("button", { name: /^Your progress/ }).click();
      await page.screenshot({ path: `${dir}/files-progress-menu-${colorScheme}-${width}x${height}.png` });
      await page.goto(`/analyses/${ANALYSIS_ID}#f1.5`);
      await expect(page.getByRole("region", { name: "Why this flow" })).toBeVisible();
      await page.waitForLoadState("networkidle");
      await page.screenshot({ path: `${dir}/flows-why-strip-${colorScheme}-${width}x${height}.png` });
      await page.goto(`/analyses/${ANALYSIS_ID}?scenario=flow-failed#f1.5`);
      await page.getByRole("button", { name: /^Your progress/ }).click();
      await expect(page.getByRole("button", { name: "Analyse flow 3 again" })).toBeVisible();
      await page.screenshot({ path: `${dir}/flows-progress-failed-${colorScheme}-${width}x${height}.png` });
    }
  }
});
