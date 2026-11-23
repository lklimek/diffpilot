import { expect, type Page } from "@playwright/test";

/** Pauses shortly ahead of the installed clock, including before navigation. */
export async function pauseClock(page: Page): Promise<void> {
  const now = await page.evaluate(() => Date.now());
  await page.clock.pauseAt(now + 100);
}

/** Flushes asynchronous UI updates without leaving the mock's current scripted phase. */
export async function settle(page: Page, check: (quick: typeof expect) => Promise<void>): Promise<void> {
  const quick = expect.configure({ timeout: 200 });
  await expect(async () => {
    // Playwright schedules nested zero-delay timers at least 1 ms ahead.
    await page.clock.runFor(1);
    await check(quick);
  }).toPass({ timeout: 10_000, intervals: [50] });
}
