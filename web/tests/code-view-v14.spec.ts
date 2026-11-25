import { expect, test } from "./fixtures";
import { ANALYSIS_ID } from "./support";

const review = `/analyses/${ANALYSIS_ID}`;
const rpc = "src/rpc/client.rs";

test("U48: line GitHub links follow the diff side, expose a tab stop and support g", async ({
  page,
  context,
}) => {
  await context.route("https://github.com/**", (route) => route.fulfill({ body: "GitHub" }));
  await page.goto(`${review}#f1.2`);
  const code = page.getByRole("region", { name: /^src\/rpc\/client\.rs, lines 88 to 96/ });
  const line = code.getByRole("button", { name: "Comment on line 89", exact: true });
  const link = code.getByRole("link", { name: `Open line 89 of ${rpc} on GitHub, new tab` });
  await expect(line).toBeVisible();
  await expect(link).toBeHidden();
  await line.hover();
  await expect(link).toBeVisible();
  await expect(link).toHaveAttribute("href", /\/pull\/482\/changes#diff-[a-f0-9]{64}R89$/);
  await page.mouse.move(0, 0);
  await line.focus();
  await page.keyboard.press("Tab");
  await expect(link).toBeFocused();
  const popup = page.waitForEvent("popup");
  await page.keyboard.press("g");
  await expect(await popup).toHaveURL(/\/changes#diff-[a-f0-9]{64}R89$/);
  const contextLine = code.getByRole("button", { name: "Comment on line 95", exact: true });
  await contextLine.focus();
  const blob = code.getByRole("link", { name: `Open line 95 of ${rpc} on GitHub, new tab` });
  await expect(blob).toHaveAttribute("href", /\/blob\/[a-f0-9]+\/src\/rpc\/client\.rs#L95$/);
  const removed = code.getByRole("button", { name: /^Comment on removed line/ }).first();
  await removed.focus();
  await expect(removed.locator("..").getByRole("link")).toHaveAttribute(
    "href",
    /\/changes#diff-[a-f0-9]{64}L\d+$/,
  );
});

test("U49: the review bar shows the current file and keeps its badges at 1280 and on a phone", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${review}?scenario=code-view-v14#f1.2`);
  const banner = page.getByRole("banner");
  const path = banner.getByLabel(`Current file: ${rpc}`);
  await expect(path).toBeVisible();
  const badges = [
    banner.getByRole("link", { name: /^Check runs:/ }),
    banner.getByText("EN", { exact: true }),
    banner.getByText("52d2da1", { exact: true }),
    banner.getByRole("button", { name: /^Your progress/ }),
    banner.getByRole("button", { name: /^Pending review/ }),
  ];
  for (const badge of badges) await expect(badge).toBeInViewport({ ratio: 1 });
  const title = banner.locator(".review-pr-title");
  expect((await title.boundingBox())?.width).toBeGreaterThan(95);
  await page.keyboard.press("k");
  await expect(banner.getByLabel("Current file: src/mempool/mod.rs")).toBeVisible();
  await page.keyboard.press("k");
  await expect(banner.getByLabel(/^Current file:/)).toHaveCount(0);
  await page.setViewportSize({ width: 390, height: 800 });
  await page.goto(`${review}/files#lines?file=src%2Frpc%2Fclient.rs&side=head`);
  await expect(path).toBeInViewport();
  await expect(banner.getByRole("button", { name: /^View: Files/ })).toBeInViewport();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(390);
});

test("U50/U51: doc context is shown automatically; range folds repeat and stop at a neighbour", async ({
  page,
}) => {
  await page.goto(`${review}?scenario=code-view-v14#f1.2`);
  const main = page.getByRole("main");
  const hideDoc = main.getByRole("button", { name: "Hide doc comment and attributes" });
  await expect(hideDoc).toBeVisible();
  const doc = main.locator('[data-line="85"][data-context="leading"]');
  const item = main.locator('[data-line="88"][data-side="head"]');
  await expect(doc).toContainText("/// Builds an RPC client.");
  expect((await doc.boundingBox())?.x).toBe((await item.boundingBox())?.x);
  await hideDoc.click();
  await expect(doc).toHaveCount(0);
  const showDoc = main.getByRole("button", { name: "Show doc comment and attributes" });
  await expect(showDoc).toBeFocused();
  await showDoc.press("Space");
  await expect(doc).toBeVisible();
  const first = main.getByRole("button", { name: "Show 10 more lines above (75–84)" });
  await first.press("Enter");
  const second = main.getByRole("button", { name: "Show 10 more lines above (65–74)" });
  await expect(second).toBeFocused();
  await expect(main.locator('[data-line="75"][data-context="expanded"]')).toBeVisible();
  await second.press("Space");
  await expect(main.locator('[data-line="65"][data-context="expanded"]')).toBeVisible();
  await main.getByRole("button", { name: "Hide added lines" }).click();
  await expect(main.locator('[data-line="65"][data-context="expanded"]')).toHaveCount(0);
  await expect(first).toBeFocused();
  await main.getByRole("link", { name: "Step 1.3 continues below" }).click();
  await expect(page).toHaveURL(/#f1\.3$/);
  await expect(main.getByRole("button", { name: "Hide doc comment and attributes" })).toHaveCount(0);
});

test("U52: a mention in an LLM remark opens and reveals a folded Files range", async ({ page }) => {
  await page.goto(`${review}?scenario=code-view-v14#f1.2`);
  const links = page.getByRole("link", { name: "rpc/client.rs:12-14", exact: true });
  const marker = page.getByRole("main").locator("[data-mark=remark]").first();
  await marker.click();
  await expect(links.first()).toHaveAttribute(
    "href",
    /\/files#lines\?file=src%2Frpc%2Fclient\.rs&side=head&line=12&end=14$/,
  );
  await links.first().click();
  await expect(page.locator('[data-line="12"][data-selected-line]')).toBeInViewport();
  await expect(page.locator('[data-line="14"][data-selected-line]')).toBeVisible();
});
