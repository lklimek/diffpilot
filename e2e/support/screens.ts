// Screens for the visual conformance check (plan §12.5): light and dark at 1440 and 3440 px when SCREENSHOT_DIR is
// set, e.g. SCREENSHOT_DIR=/data/artifacts/smart-review/<date>/<name>; nothing otherwise.
import { join } from "node:path";
import type { Page } from "@playwright/test";

const SCREENSHOT_DIR = process.env.SCREENSHOT_DIR;
const VIEWS = [
  { width: 1440, height: 900 },
  { width: 3440, height: 1440 },
] as const;

/** Saves `<name>-<scheme>-<width>.png` of the page; the viewport and the light scheme are restored. */
export async function screens(page: Page, name: string): Promise<void> {
  if (!SCREENSHOT_DIR) return;
  const viewport = page.viewportSize();
  for (const colorScheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme });
    for (const view of VIEWS) {
      await page.setViewportSize(view);
      await page.screenshot({ path: join(SCREENSHOT_DIR, `${name}-${colorScheme}-${view.width}.png`) });
    }
  }
  await page.emulateMedia({ colorScheme: "light" });
  if (viewport) await page.setViewportSize(viewport);
}
