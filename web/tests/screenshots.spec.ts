import { test } from "./fixtures";
import { openTour } from "./support";

// Light and dark screenshots of the three blocks for the visual comparison with docs/mockups (plan §12.5).
// Runs only with SCREENSHOT_DIR set, e.g. SCREENSHOT_DIR=/data/artifacts/smart-review/<date>/wp2-web.
const dir = process.env.SCREENSHOT_DIR;

for (const colorScheme of ["light", "dark"] as const) {
  for (const [width, height] of [
    [1440, 900],
    [3440, 1440],
  ] as const) {
    test(`blocks ${colorScheme} ${width}x${height}`, async ({ browser }) => {
      test.skip(!dir, "SCREENSHOT_DIR not set");
      const page = await browser.newPage({ colorScheme, viewport: { width, height } });
      await openTour(page);
      const shot = (name: string) => page.screenshot({ path: `${dir}/${name}-${colorScheme}-${width}.png` });
      await shot("pr-context");
      await page.keyboard.press("j");
      await shot("flow-intro");
      await page.keyboard.press("j");
      await page.getByRole("region", { name: /lines 126 to 132/ }).waitFor();
      await shot("step1");
      await page.getByRole("navigation").getByRole("button", { name: "Step 5: Retry allowed?" }).click();
      await page.getByRole("region", { name: /lines 99 to 99/ }).waitFor();
      await shot("step5");
      await page.close();
    });
  }
}
