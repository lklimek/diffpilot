import { expect, test } from "./fixtures";
import { ANALYSIS_ID } from "./support";

const review = `/pr/acme/node-client/482/analyses/${ANALYSIS_ID}`;

test("flow context can be pinned independently while the code scrolls", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 620 });
  await page.goto(`${review}#f1.2`);
  const main = page.getByRole("main");
  const diagram = main.getByRole("region", { name: "Flow diagram" });
  const why = main.getByRole("region", { name: "Why this flow" });
  const pinWhy = why.getByRole("button", { name: "Pin Why this flow" });
  const pinDiagram = diagram.getByRole("button", { name: "Pin flow diagram" });
  await pinWhy.press("Enter");
  await expect(pinWhy).toHaveAttribute("aria-pressed", "true");
  await expect(pinWhy).toBeFocused();
  await diagram.getByRole("button", { name: /Show flow diagram/ }).click();
  await pinDiagram.click();
  const scroller = main.locator("[data-scroll-column]");
  const top = (await scroller.boundingBox())!.y;
  expect((await scroller.boundingBox())!.height).toBeGreaterThan(200);
  expect((await diagram.boundingBox())!.y).toBeLessThan(top);
  expect((await why.boundingBox())!.y).toBeLessThan(top);
  const scroll = await scroller.evaluate((el) => {
    el.scrollTop = 10_000;
    return el.scrollTop;
  });
  expect(scroll).toBeGreaterThan(0);
  expect((await diagram.boundingBox())!.y).toBeLessThan(top);
  await page.reload();
  await expect(pinDiagram).toHaveAttribute("aria-pressed", "true");
  await pinDiagram.press("Space");
  await expect(pinDiagram).toHaveAttribute("aria-pressed", "false");
  await expect(pinDiagram).toBeFocused();
  await expect(scroller.getByRole("region", { name: "Flow diagram" })).toBeAttached();
  await expect(scroller.getByRole("region", { name: "Why this flow" })).toHaveCount(0);
  expect((await why.boundingBox())!.y).toBeLessThan((await scroller.boundingBox())!.y);
});

test("step checkboxes track the existing review state without navigating", async ({ page }) => {
  await page.goto(`${review}#f1.2`);
  const sidebar = page.getByRole("navigation", { name: "Flows and steps" });
  const first = sidebar.getByRole("checkbox", { name: /Mark step 1 reviewed/ });
  const second = sidebar.getByRole("checkbox", { name: /Mark step 2 reviewed/ });
  await expect(first).not.toBeChecked();
  const saved = page.waitForResponse(
    (response) => response.url().endsWith("/review-state") && response.request().method() === "PUT",
  );
  await first.press("Space");
  await expect(first).toBeChecked();
  expect((await saved).ok()).toBe(true);
  await expect(page).toHaveURL(/#f1\.2$/);
  await page.reload();
  await expect(first).toBeChecked();
  await first.click();
  await expect(first).not.toBeChecked();
  await sidebar.getByRole("button", { name: /^Step 1:/ }).click();
  await expect(page).toHaveURL(/#f1\.1$/);
  await page
    .getByRole("main")
    .getByRole("button", { name: /Mark reviewed/ })
    .click();
  await expect(first).toBeChecked();
  await expect(second).not.toBeChecked();
});

test("clipped impact explanations expand independently of their code preview", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 750 });
  await page.goto(`${review}#f2.1`);
  const impact = page.getByRole("region", { name: "Impact outside the diff" });
  const explanation = impact.getByRole("button", { name: /The failing CI test/ });
  const code = impact.getByRole("button", { name: /tests\/sync_headers\.rs.*8–14/ });
  await expect(explanation).toHaveAttribute("aria-expanded", "false");
  await explanation.press("Enter");
  await expect(explanation).toHaveAttribute("aria-expanded", "true");
  const details = impact.locator(`[id="${await explanation.getAttribute("aria-controls")}"]`);
  await expect(details).toContainText("longer than the new 5 s timeout");
  await expect(details).toContainText("(fact)");
  await expect(code).toHaveAttribute("aria-expanded", "false");
  await code.click();
  await expect(code).toHaveAttribute("aria-expanded", "true");
  await explanation.press("Space");
  await expect(explanation).toHaveAttribute("aria-expanded", "false");
  await expect(code).toHaveAttribute("aria-expanded", "true");
  await page.setViewportSize({ width: 1440, height: 750 });
  await expect(explanation).toHaveCount(0);
  await expect(impact.getByText(/The failing CI test/)).toBeVisible();
});

test("the review title takes extra width before a long repository label", async ({ page }) => {
  await page.setViewportSize({ width: 1200, height: 800 });
  await page.goto(`${review}?scenario=code-view-v14-long-title#f1.2`);
  const banner = page.getByRole("banner");
  const title = banner.locator(".review-pr-title");
  const widthOfTitle = async () => (await title.boundingBox())!.width;
  await expect(title).toBeInViewport({ ratio: 1 });
  const start = await widthOfTitle();
  await page.setViewportSize({ width: 1600, height: 800 });
  await expect.poll(widthOfTitle).toBeGreaterThan(start + 75);
  await expect(banner.getByRole("link", { name: /^CI:/ })).toBeInViewport({ ratio: 1 });
});
