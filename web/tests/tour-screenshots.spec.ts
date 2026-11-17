import { test, type Page } from "@playwright/test";
import { openTour } from "./support";

// The six fixture states in light and dark at 1440 and 3440 px for the comparison with docs/mockups
// (plan §12.5). Runs only with SCREENSHOT_DIR set, e.g. SCREENSHOT_DIR=/data/artifacts/smart-review/<date>/wp3-tour.
const dir = process.env.SCREENSHOT_DIR;

const nav = (page: Page) => page.getByRole("navigation");
const inspector = (page: Page) => page.getByRole("complementary", { name: "Inspector" });

// What each state shows (as the mockup's state switcher does).
const STATES: Record<string, (page: Page) => Promise<void>> = {
  default: async () => {},
  wrong: async (page) => {
    await nav(page)
      .getByRole("button", { name: /^Flow 3:/ })
      .click();
    await nav(page)
      .getByRole("button", { name: /^Step 1:/ })
      .click();
  },
  missed: async (page) => {
    await nav(page)
      .getByRole("button", { name: /^Flow 1:/ })
      .click();
    await nav(page)
      .getByRole("button", { name: /^Step 2:/ })
      .click();
    await page.getByRole("button", { name: "Comment on line 96" }).click();
  },
  noq: async (page) => {
    await nav(page)
      .getByRole("button", { name: /^Flow 1:/ })
      .click();
    await nav(page)
      .getByRole("button", { name: /^Step 5:/ })
      .click();
    await inspector(page).getByRole("button", { name: "Questions to check" }).click();
  },
  noprofile: async () => {},
  resume: async () => {},
};

for (const [state, show] of Object.entries(STATES)) {
  for (const colorScheme of ["light", "dark"] as const) {
    for (const [width, height] of [
      [1440, 900],
      [3440, 1440],
    ] as const) {
      test(`tour ${state} ${colorScheme} ${width}x${height}`, async ({ browser }) => {
        test.skip(!dir, "SCREENSHOT_DIR not set");
        const page = await browser.newPage({ colorScheme, viewport: { width, height } });
        await openTour(page, state);
        await show(page);
        await page.waitForTimeout(300);
        await page.screenshot({ path: `${dir}/tour-${state}-${colorScheme}-${width}.png` });
        await page.close();
      });
    }
  }
}
