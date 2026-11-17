import { test, type Page } from "@playwright/test";
import { ANALYSIS_ID, openTour } from "./support";

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

async function openStep(page: Page, flow: number, step: number) {
  await nav(page)
    .getByRole("button", { name: new RegExp(`^Flow ${flow}:`) })
    .click();
  await nav(page)
    .getByRole("button", { name: new RegExp(`^Step ${step}:`) })
    .click();
}

// The states visual conformance check 2 asked for (columns, diagram, old code, missed note, focus, tooltip).
const EXTRA: Record<string, [string, (page: Page) => Promise<void>]> = {
  "columns-hidden": [
    "default",
    async (page) => {
      await page.getByRole("button", { name: "Hide Flows column" }).click();
      await page.getByRole("button", { name: "Show Flows column" }).hover();
    },
  ],
  "columns-resized": [
    "default",
    async (page) => {
      const separator = page.getByRole("separator", { name: /Resize the Inspector column/ });
      await separator.focus();
      for (let i = 0; i < 6; i += 1) await page.keyboard.press("ArrowLeft");
    },
  ],
  "columns-fullscreen": [
    "default",
    async (page) => {
      await page.getByRole("button", { name: "Full screen Review column" }).click();
    },
  ],
  "diagram-open": [
    "default",
    async (page) => {
      await openStep(page, 1, 5);
      await page.getByRole("button", { name: "Show flow diagram" }).click();
      await page
        .getByRole("group", { name: /^Flow diagram:/ })
        .first()
        .waitFor();
    },
  ],
  "old-code": [
    "default",
    async (page) => {
      await openStep(page, 1, 2);
      await page.getByRole("button", { name: "Show old code" }).click();
    },
  ],
  "missed-note": [
    "default",
    async (page) => {
      await openStep(page, 1, 2);
      await page.getByRole("button", { name: "Comment on line 96" }).click();
      await page.getByRole("textbox", { name: /comment/i }).fill("Each attempt gets the full timeout.");
      await page.getByRole("button", { name: "Add to GitHub pending review" }).click();
      await page.getByText(/Also recorded as missed by the LLM/).waitFor();
    },
  ],
  "feedback-memory": [
    "wrong",
    async (page) => {
      await openStep(page, 3, 1);
      await page.getByRole("list", { name: "Calibration memory" }).waitFor();
    },
  ],
  "line-feedback": [
    "default",
    async (page) => {
      await openStep(page, 1, 2);
      await page.getByRole("button", { name: "Comment on line 96" }).click();
      await page.getByRole("group", { name: "Feedback on line 96" }).waitFor();
    },
  ],
  "unchecked-facts": [
    "default",
    async (page) => {
      await page.goto(`/analyses/${ANALYSIS_ID}?scenario=unchecked-facts`);
      await page.getByText("Dependency files not checked: Cargo.lock").first().waitFor();
    },
  ],
  "focus-visible": [
    "default",
    async (page) => {
      await openStep(page, 1, 5);
      await page.locator("body").click({ position: { x: 1, y: 1 } });
      for (let i = 0; i < 4; i += 1) await page.keyboard.press("Tab");
    },
  ],
  "label-tooltip": [
    "default",
    async (page) => {
      await openStep(page, 1, 5);
      await page
        .getByRole("main")
        .getByRole("button", { name: /HIGH · LLM remark/ })
        .hover();
      await page.getByRole("tooltip").first().waitFor();
    },
  ],
};

const STEPS: [string, number, number][] = [
  ["f1.1", 1, 1],
  ["f1.2", 1, 2],
  ["f1.5", 1, 5],
  ["f3.1", 3, 1],
];

for (const colorScheme of ["light", "dark"] as const) {
  for (const [width, height] of [
    [1440, 900],
    [3440, 1440],
  ] as const) {
    for (const [name, [state, show]] of Object.entries(EXTRA)) {
      test(`tour ${name} ${colorScheme} ${width}x${height}`, async ({ browser }) => {
        test.skip(!dir, "SCREENSHOT_DIR not set");
        const page = await browser.newPage({ colorScheme, viewport: { width, height } });
        await openTour(page, state);
        await show(page);
        await page.waitForTimeout(300);
        await page.screenshot({ path: `${dir}/tour-${name}-${colorScheme}-${width}.png` });
        await page.close();
      });
    }
    for (const [id, flow, step] of STEPS) {
      test(`step block ${id} ${colorScheme} ${width}x${height}`, async ({ browser }) => {
        test.skip(!dir, "SCREENSHOT_DIR not set");
        const page = await browser.newPage({ colorScheme, viewport: { width, height } });
        await openTour(page);
        await openStep(page, flow, step);
        await page.waitForTimeout(300);
        await page
          .locator("[data-column=main]")
          .screenshot({ path: `${dir}/step-${id}-${colorScheme}-${width}.png` });
        await page.close();
      });
    }
  }
}
