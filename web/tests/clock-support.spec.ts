import { setTimeout as delay } from "node:timers/promises";
import { pauseClock } from "./clock-support";
import { expect, test } from "./fixtures";

for (const navigate of [false, true]) {
  test(`pauseClock tolerates a delayed command, navigation=${navigate}`, async ({ page }) => {
    if (navigate) await page.goto("/");
    const pauseAt = page.clock.pauseAt.bind(page.clock);
    page.clock.pauseAt = async (time) => {
      await delay(250);
      await pauseAt(time);
    };
    await pauseClock(page);
    const paused = await page.evaluate(() => Date.now());
    await delay(250);
    expect(await page.evaluate(() => Date.now())).toBe(paused);
    await page.clock.runFor(1_000);
    expect(await page.evaluate(() => Date.now())).toBe(paused + 1_000);
  });
}
