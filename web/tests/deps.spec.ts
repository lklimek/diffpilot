import AxeBuilder from "@axe-core/playwright";
import { type Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import { ANALYSIS_ID, expectTrustWording } from "./support";

// The Dependencies view (brief 2026-11-20, plan U1–U5) against the msw mocks of fixtures/pr-482. Screenshots of
// every scenario only with SCREENSHOT_DIR set, e.g. SCREENSHOT_DIR=/data/artifacts/diffpilot/<date>/go9-web.
const DEPS = `/analyses/${ANALYSIS_ID}/deps`;
const shots = process.env.SCREENSHOT_DIR;
const SCENARIOS = [
  "deps-multi",
  "deps-limits",
  "deps-not-rust",
  "deps-error",
  "deps-notes-pending",
  "deps-notes-not-produced",
  "deps-notes-empty",
  "deps-no-controlled",
  "deps-no-changed-crate",
];
const WIDTHS = [1600, 760];

async function openDeps(page: Page, scenario: string | null, width = 1600) {
  await page.setViewportSize({ width, height: 900 });
  await page.goto(scenario ? `${DEPS}?scenario=${scenario}` : DEPS);
  await expect(page.getByRole("heading", { level: 1, name: "Dependencies" })).toBeVisible();
}

/** Opens every closed level, deepest last, so the whole tree is on the page. */
async function openAll(page: Page) {
  const tree = page.getByRole("list", { name: "Repositories" });
  if ((await tree.count()) === 0) return;
  for (let i = 0; i < 50; i++) {
    const closed = tree.locator("button[data-disclosure][aria-expanded=false]");
    if ((await closed.count()) === 0) return;
    await closed.first().click();
  }
}

async function seriousViolations(page: Page, width: number) {
  const results = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"])
    .analyze();
  // Below 1000 px the shared top bar crowds its controls in every review view (Files too): not this view's.
  const sharedBar = (id: string, html: string) =>
    width < 1000 && id === "target-size" && /aria-label="(View: )/.test(html);
  // The heading outline (best practice, any impact): no skipped level.
  const headings = await new AxeBuilder({ page }).withRules(["heading-order"]).analyze();
  return [
    ...results.violations.filter((v) => v.impact === "serious" || v.impact === "critical"),
    ...headings.violations,
  ]
    .map((v) => ({ ...v, nodes: v.nodes.filter((n) => !sharedBar(v.id, n.html)) }))
    .filter((v) => v.nodes.length > 0)
    .map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(" ")}`);
}

test("no serious axe violations in any state, wide and narrow, light and dark; trust wording holds", async ({
  page,
}) => {
  test.setTimeout(180_000);
  for (const scenario of SCENARIOS) {
    for (const width of WIDTHS) {
      for (const colorScheme of ["light", "dark"] as const) {
        await page.emulateMedia({ colorScheme });
        await openDeps(page, scenario, width);
        await expect(
          page
            .getByRole("list", { name: "Repositories" })
            .or(page.getByRole("alert"))
            .or(page.getByText(/^Dependencies: not available/)),
        ).toBeVisible();
        await openAll(page);
        expect(await seriousViolations(page, width), `${scenario} ${width} ${colorScheme}`).toEqual([]);
        await expectTrustWording(page);
        if (shots && colorScheme === "light") {
          await page.evaluate(() => window.scrollTo(0, 0));
          await page.screenshot({ path: `${shots}/${scenario}-${width}.png`, fullPage: true });
        }
      }
    }
  }
});

test("the page fits narrow widths without horizontal scrolling", async ({ page }) => {
  await openDeps(page, "deps-limits", 760);
  await openAll(page);
  const sizes = await page.evaluate(() => ({
    width: document.documentElement.scrollWidth,
    client: document.documentElement.clientWidth,
  }));
  expect(sizes.width).toBeLessThanOrEqual(sizes.client);
});

test("keyboard: levels toggle with Enter and Space and show a focus ring", async ({ page }) => {
  await openDeps(page, "deps-multi");
  const sdk = page.getByRole("button", { name: /^acme\/node-sdk/ });
  await sdk.focus();
  await expect(sdk).toHaveAttribute("aria-expanded", "false");
  await page.keyboard.press("Enter");
  await expect(sdk).toHaveAttribute("aria-expanded", "true");
  await expect(page.getByRole("button", { name: /^rpc-types/ })).toBeVisible();
  const outline = await sdk.evaluate((el) => getComputedStyle(el).outlineStyle);
  expect(outline).not.toBe("none");
  await page.keyboard.press("Space");
  await expect(sdk).toHaveAttribute("aria-expanded", "false");
  // The open state survives a reload.
  await page.keyboard.press("Space");
  await page.reload();
  await expect(page.getByRole("button", { name: /^acme\/node-sdk/ })).toHaveAttribute(
    "aria-expanded",
    "true",
  );
});

test("a fact's line opens the Files view at that line; the breadcrumb leads back", async ({ page }) => {
  await openDeps(page, "deps-multi");
  await page.getByRole("link", { name: "src/rpc/error.rs:12" }).click();
  await expect(page).toHaveURL(/\/files#lines\?file=src%2Frpc%2Ferror\.rs&side=head&line=12$/);
  await expect(page.getByRole("main").locator("h2:not([data-column-title])").first()).toHaveText(
    "src/rpc/error.rs",
  );
  const crumbs = page.getByRole("navigation", { name: "Breadcrumb" });
  await crumbs.getByRole("button", { name: "View: Files" }).click();
  await crumbs.getByRole("link", { name: "Dependencies" }).click();
  await expect(page).toHaveURL(new RegExp(`${DEPS}$`));
  await expect(page.getByRole("heading", { level: 1, name: "Dependencies" })).toBeVisible();
});

test("the facts say what the token rules cover; LLM notes carry the LLM label", async ({ page }) => {
  await openDeps(page, null);
  const crate = page.getByRole("button", { name: /^node-client/ }).locator("xpath=ancestor::li[1]");
  await expect(crate).toContainText("No finding here is not evidence that none exist.");
  const note = crate.getByRole("listitem").filter({ hasText: "retry with backoff" });
  await expect(note.locator("[data-provenance=llm]")).toHaveAccessibleDescription(/written by the LLM/);
  await expect(crate).toContainText("+3 more notes, not listed.");
});
