import type { Locator } from "@playwright/test";
import { expect, test } from "./fixtures";
import { ANALYSIS_ID } from "./support";

const review = `/analyses/${ANALYSIS_ID}`;
const rpc = "src/rpc/client.rs";

async function expectCodeLineHeight(row: Locator) {
  await expect(row).toBeVisible();
  const [height, lineHeight] = await row.evaluate((element) => [
    element.getBoundingClientRect().height,
    parseFloat(getComputedStyle(element).lineHeight),
  ]);
  expect(height).toBeCloseTo(lineHeight!, 1);
}

test("keeps code density in Flows, Files and the classic diff", async ({ page }) => {
  await page.goto(`${review}#f1.2`);
  const main = page.getByRole("main");
  const row = main.locator('[data-line="95"][data-side="head"]');
  await expectCodeLineHeight(row);
  await row.hover();
  await expectCodeLineHeight(row);
  await page.keyboard.press("d");
  const diff = main.getByRole("region", { name: `Classic diff of ${rpc}` });
  await expectCodeLineHeight(diff.locator('[data-diff-line="context"]').first());
  await page.goto(`${review}/files#lines?file=src%2Frpc%2Fclient.rs&side=head`);
  await expectCodeLineHeight(main.locator('[data-line="95"][data-side="head"]'));
});

test("the classic diff number is the only GitHub tab stop on its line", async ({ page, context }) => {
  await context.route("https://github.com/**", (route) => route.fulfill({ body: "GitHub" }));
  await page.goto(`${review}#f1.2`);
  await expect(page.getByRole("main").getByRole("button", { name: "Comment on line 89" })).toBeVisible();
  await page.keyboard.press("d");
  const diff = page.getByRole("main").getByRole("region", { name: `Classic diff of ${rpc}` });
  const row = diff.locator('[data-ask-side="head"][data-ask-number="89"]');
  const number = row.getByRole("button", { name: "Open line 89 on GitHub, new tab" });
  await number.focus();
  const icon = row.getByRole("link", { name: `Open line 89 of ${rpc} on GitHub, new tab` });
  await expect(icon).toBeVisible();
  await page.keyboard.press("Tab");
  await expect(row.getByRole("button", { name: "Ask the LLM about line 89" })).toBeFocused();
  await expect(icon).toHaveAttribute("tabindex", "-1");
  await page.keyboard.press("Shift+Tab");
  await expect(number).toBeFocused();
  const popup = page.waitForEvent("popup");
  await number.click();
  const github = await popup;
  await expect(github).toHaveURL(/\/changes#diff-[a-f0-9]{64}R89$/);
  expect(await github.evaluate(() => window.opener === null)).toBe(true);
  await github.close();
});

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
  expect(
    await link.evaluate((element) => {
      const hitArea = getComputedStyle(element, "::before");
      const { left, top, height } = element.getBoundingClientRect();
      const hitsGutter = document.elementFromPoint(left + 22, top + height / 2) === element;
      return [hitArea.width, hitArea.height, hitsGutter];
    }),
  ).toEqual(["24px", "24px", true]);
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

const desktopWidths = [
  900, 1000, 1100, 1199, 1200, 1280, 1366, 1439, 1440, 1599, 1600, 1601, 1823, 1824, 1825, 1920,
];

async function expectReadableTitle(title: Locator, width: number) {
  if (width < 1200 && !(await title.isVisible())) {
    await expect(title).toBeHidden();
    return;
  }
  await expect(title).toBeVisible();
  const { available, readable } = await title.evaluate((el) => {
    const context = document.createElement("canvas").getContext("2d")!;
    context.font = getComputedStyle(el).font;
    return {
      available: el.getBoundingClientRect().width,
      readable: context.measureText("0".repeat(16)).width,
    };
  });
  expect(available).toBeGreaterThanOrEqual(readable - 1);
}

async function expectInlineOwner(owner: Locator, repo: Locator) {
  await expect(owner).toBeInViewport({ ratio: 1 });
  expect(await owner.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
  const ownerBox = (await owner.boundingBox())!;
  const repoBox = (await repo.boundingBox())!;
  expect(Math.abs(ownerBox.y + ownerBox.height / 2 - repoBox.y - repoBox.height / 2)).toBeLessThan(2);
}

async function checkReviewLabelWidths(title: Locator, repo: Locator, previous = { title: 0, repo: 0 }) {
  const visibleWidth = async (label: Locator) =>
    (await label.isVisible()) ? Math.round((await label.boundingBox())?.width ?? 0) : 0;
  const widths = {
    title: await visibleWidth(title),
    repo: await visibleWidth(repo),
  };
  expect(widths.title).toBeGreaterThanOrEqual(previous.title);
  expect(widths.repo).toBeGreaterThanOrEqual(previous.repo);
  return widths;
}

test("U49: the review bar shows the current file and keeps its badges on desktop and on a phone", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${review}?scenario=code-view-v14#f1.2`);
  const banner = page.getByRole("banner");
  const path = banner.getByLabel(`Current file: ${rpc}`);
  await expect(path).toBeVisible();
  await path.focus();
  await expect(path).toBeFocused();
  await expect(banner.getByRole("tooltip", { name: rpc, exact: true })).toBeVisible();
  await expect(path).toHaveAccessibleDescription(rpc);
  await expect(path.locator("xpath=ancestor-or-self::*[@aria-live]")).toHaveCount(0);
  const checks = banner.getByRole("link", {
    name: /^CI: 3 passed, 1 failed, 1 pending, 1 skipped, 1 neutral/,
  });
  await expect(checks).toContainText("CI: 3 passed, 1 failed, 1 pending, 1 skipped, 1 neutral");
  await expect(checks).toHaveAccessibleDescription(/Check runs: .*\. Statuses: 1 pending\./);
  const badges = [
    checks,
    banner.getByText("EN", { exact: true }),
    banner.getByText("52d2da1", { exact: true }),
    banner.getByRole("button", { name: /^Your progress/ }),
    banner.getByRole("button", { name: /^Pending review/ }),
    banner.getByRole("button", { name: /account and LLM activity/ }),
  ];
  const title = banner.locator(".review-pr-title");
  const crumbs = banner.getByRole("navigation", { name: "Breadcrumb" });
  await expect(crumbs.getByRole("link", { name: "Pull requests" })).toHaveAttribute("href", "/prs");
  const repository = "acme/rust-dashcore-platform-integration";
  const pr = crumbs.getByRole("link", { name: `${repository} #482` });
  const repo = pr.locator(".review-repo-name");
  await expect(pr).toHaveAttribute("href", `/pr/${repository}/482`);
  await expect(pr).toHaveAccessibleDescription(`${repository} #482`);
  let previous = { title: 0, repo: 0 };
  for (const width of desktopWidths) {
    await test.step(`${width} px`, async () => {
      await page.setViewportSize({ width, height: 800 });
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width);
      await expect(path).toBeInViewport({ ratio: 1 });
      for (const badge of badges) await expect(badge).toBeInViewport({ ratio: 1 });
      if (width >= 1200) await expect(repo).toBeInViewport({ ratio: 1 });
      previous = await checkReviewLabelWidths(title, repo, previous);
      if (width >= 1200) expect(previous.repo).toBeGreaterThan(45);
      const counts = checks.locator(".review-ci-counts");
      expect(await counts.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
      await expectReadableTitle(title, width);
      const compactCaption = width < 1824;
      const caption = banner.getByText(compactCaption ? "Hide imports" : "Hide comment and import changes", {
        exact: true,
      });
      await expect(caption).toBeInViewport({ ratio: 1 });
      expect((await caption.boundingBox())?.width).toBeGreaterThan(20);
      const checkbox = compactCaption
        ? "Hide imports and comment changes"
        : "Hide comment and import changes";
      await expect(banner.getByRole("checkbox", { name: checkbox })).toBeChecked();
    });
  }
  await page.keyboard.press("k");
  await expect(banner.getByLabel("Current file: src/mempool/mod.rs")).toBeVisible();
  await page.keyboard.press("k");
  await expect(banner.getByLabel(/^Current file:/)).toHaveCount(0);
  await page.setViewportSize({ width: 390, height: 800 });
  await page.goto(`${review}/files#lines?file=src%2Frpc%2Fclient.rs&side=head`);
  await expect(path).toBeInViewport();
  await expect(title).toBeHidden();
  await expect(banner.getByRole("button", { name: /^View: Files/ })).toBeInViewport();
  for (const badge of badges) {
    await badge.scrollIntoViewIfNeeded();
    await expect(badge).toBeInViewport({ ratio: 1 });
  }
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(390);
});

test("U49/U62: a long PR title truncates without pushing review controls out of view", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto(`${review}?scenario=code-view-v14-long-title#f1.2`);
  const banner = page.getByRole("banner");
  const title = banner.locator(".review-pr-title");
  const crumbs = banner.getByRole("navigation", { name: "Breadcrumb" });
  const repo = crumbs.locator(".review-repo-name");
  const owner = crumbs.locator(".review-owner");
  const fixedCrumbs = [
    crumbs.getByRole("link", { name: "diffpilot" }),
    crumbs.getByRole("link", { name: "Pull requests" }),
  ];
  const checks = banner.getByRole("link", {
    name: /^CI: 3 passed, 1 failed, 1 pending, 1 skipped, 1 neutral/,
  });
  const controls = [
    ...fixedCrumbs,
    banner.getByLabel(`Current file: ${rpc}`),
    banner.getByText("#482", { exact: true }),
    checks,
    banner.getByText("EN", { exact: true }),
    banner.getByText("52d2da1", { exact: true }),
    banner.getByRole("button", { name: /^Your progress/ }),
    banner.getByRole("button", { name: /^Pending review/ }),
    banner.getByRole("button", { name: /account and LLM activity/ }),
  ];
  await expect(title).toHaveText(/^.{100,}$/);
  let previous = { title: 0, repo: 0 };
  for (const width of desktopWidths) {
    await test.step(`${width} px`, async () => {
      await page.setViewportSize({ width, height: 800 });
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width);
      for (const control of controls) await expect(control).toBeInViewport({ ratio: 1 });
      for (const link of fixedCrumbs) {
        expect(await link.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
      }
      expect((await banner.boundingBox())?.height).toBe(48);
      if (width >= 1200) await expect(repo).toBeInViewport({ ratio: 1 });
      if (width >= 1440) await expectInlineOwner(owner, repo);
      previous = await checkReviewLabelWidths(title, repo, previous);
      if (width >= 1200) expect(previous.repo).toBeGreaterThan(45);
      await expectReadableTitle(title, width);
      if (!previous.title) await expect(repo).toBeHidden();
      if (width === 1100) await expect(title).toBeInViewport({ ratio: 1 });
      expect(
        await checks.locator(".review-ci-counts").evaluate((el) => el.scrollWidth <= el.clientWidth),
      ).toBe(true);
      if (width >= 1200) {
        await expect(title).toHaveCSS("text-overflow", "ellipsis");
        expect(await title.evaluate((element) => element.scrollWidth > element.clientWidth)).toBe(true);
      }
      if (width === 1920) {
        const lines = await checks.locator(".review-ci-counts").evaluate((el) => {
          const text = document.createRange();
          text.selectNodeContents(el);
          return text.getClientRects().length;
        });
        expect(lines).toBe(1);
      }
    });
  }
});

test("U49/U62: the review bar fits a short repository name at 700px and stays 48px high", async ({
  page,
}) => {
  await page.setViewportSize({ width: 700, height: 800 });
  await page.goto(`${review}#f1.2`);
  const banner = page.getByRole("banner");
  const pr = banner.getByRole("link", { name: "acme/node-client #482", exact: true });
  const repo = pr.locator(".review-repo-name");
  for (const width of [700, 1200, 1440, 1920]) {
    await test.step(`${width} px`, async () => {
      await page.setViewportSize({ width, height: 800 });
      await expect(repo).toHaveText("node-client");
      await expect(repo).toBeInViewport({ ratio: 1 });
      expect(await repo.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true);
      expect((await banner.boundingBox())?.height).toBe(48);
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width);
      if (width >= 1440) await expectInlineOwner(pr.locator(".review-owner"), repo);
    });
  }
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
    `/pr/acme/node-client/482/analyses/${ANALYSIS_ID}/files#lines?file=src%2Frpc%2Fclient.rs&side=head&line=12&end=14`,
  );
  await links.first().click();
  await expect(page.locator('[data-line="12"][data-selected-line]')).toBeInViewport();
  await expect(page.locator('[data-line="14"][data-selected-line]')).toBeVisible();
});
