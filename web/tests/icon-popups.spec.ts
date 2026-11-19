import { type Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import { openTour } from "./support";

// Owner: hover or keyboard focus shows the popup, moving away hides it unless it was clicked, a click pins it until
// the next click. The icon: the "?" of line 89 in step 1.2.
const code = (page: Page) => page.getByRole("region", { name: /^src\/rpc\/client\.rs, lines/ });
const open = async (page: Page) => {
  await openTour(page);
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Flow 1:/ })
    .click();
  await page
    .getByRole("navigation")
    .getByRole("button", { name: /^Step 2:/ })
    .click();
};
const icon = (page: Page) => code(page).getByRole("button", { name: "Ask the LLM about line 89" });
const popup = (page: Page) => page.getByRole("dialog", { name: "Ask the LLM about line 89" });

test("mouse: hover shows, leaving hides, a click pins, the second click closes even with the icon focused", async ({
  page,
}) => {
  await open(page);
  await icon(page).hover();
  await expect(popup(page)).toBeVisible();
  await page.mouse.move(0, 0);
  await expect(popup(page)).toHaveCount(0);
  await icon(page).click();
  await page.mouse.move(0, 0);
  await expect(popup(page)).toBeVisible();
  await icon(page).hover();
  await icon(page).click();
  await page.mouse.move(0, 0);
  await expect(popup(page)).toHaveCount(0);
});

test("keyboard: focus shows the popup, Escape closes it", async ({ page }) => {
  await open(page);
  await code(page).getByRole("button", { name: "Comment on line 89" }).focus();
  await page.keyboard.press("Tab");
  await expect(icon(page)).toBeFocused();
  await expect(popup(page)).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(popup(page)).toHaveCount(0);
});

test.describe("touch", () => {
  test.use({ hasTouch: true });
  test("a tap pins the popup, the second tap closes it", async ({ page }) => {
    await open(page);
    await icon(page).scrollIntoViewIfNeeded();
    const box = (await icon(page).boundingBox())!;
    const at = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
    await page.touchscreen.tap(at.x, at.y);
    await expect(popup(page)).toBeVisible();
    await page.touchscreen.tap(at.x, at.y);
    await expect(popup(page)).toHaveCount(0);
  });
});
