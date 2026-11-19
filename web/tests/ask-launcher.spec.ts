import { expect, test } from "./fixtures";
import { ANALYSIS_ID } from "./support";

// The bottom-right launcher (AC1) covers nothing of the Inspector: the Inspector ends above it.
for (const [width, height] of [
  [1600, 900],
  [2560, 1080],
] as const) {
  test(`the Ask launcher leaves the Inspector and the comment composer uncovered at ${width}x${height}`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height });
    await page.goto(`/analyses/${ANALYSIS_ID}#f1.5`);
    const launcher = page.getByRole("button", { name: /^Ask.*: open your private chat/ });
    await expect(launcher).toBeVisible();
    await page.getByRole("main").getByRole("button", { name: "Comment on line 99" }).click();
    const dock = page.locator("[data-inspector] [data-dock]");
    await expect(dock.getByRole("textbox").last()).toBeVisible();
    if (process.env.SCREENSHOT_DIR)
      await page.screenshot({ path: `${process.env.SCREENSHOT_DIR}/launcher-composer-${width}.png` });
    const l = (await launcher.boundingBox())!;
    const covered: string[] = [];
    for (const control of await dock.locator("button, textarea, input, a").all()) {
      if (!(await control.isVisible())) continue;
      const b = (await control.boundingBox())!;
      const overlap =
        b.x < l.x + l.width && l.x < b.x + b.width && b.y < l.y + l.height && l.y < b.y + b.height;
      if (overlap) covered.push((await control.getAttribute("aria-label")) ?? (await control.innerText()));
    }
    expect(covered).toEqual([]);
    const d = (await dock.boundingBox())!;
    expect(d.y + d.height).toBeLessThanOrEqual(l.y);
  });
}
