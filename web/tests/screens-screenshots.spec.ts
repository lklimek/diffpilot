import { type Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import { analyze } from "./screens-support";
import { ANALYSIS_ID } from "./support";

// Light and dark screenshots of every Pull requests, progress and Settings state at 1440 and 3440 (plan §12.5).
// Runs only with SCREENSHOT_DIR set, e.g. SCREENSHOT_DIR=/data/artifacts/diffpilot/<date>/wp3-screens.
const dir = process.env.SCREENSHOT_DIR;
const progress = `/analyses/${ANALYSIS_ID}/progress`;

const STATES: [string, (page: Page) => Promise<unknown>][] = [
  ["home-signed-out", (p) => p.goto("/prs?scenario=signed-out")],
  ["home-login-not-allowed", (p) => p.goto("/prs?scenario=signed-out&login_error=login_not_allowed")],
  ["home-signed-in", (p) => p.goto("/prs")],
  [
    "home-not-a-pr",
    (p) => p.goto("/prs").then(() => analyze(p, "https://github.com/acme/node-client/issues/5")),
  ],
  [
    "home-private",
    (p) => p.goto("/prs").then(() => analyze(p, "https://github.com/acme/private-repo/pull/1")),
  ],
  ["home-resume", (p) => p.goto("/prs?state=resume").then(() => analyze(p))],
  ["home-head-moved", (p) => p.goto("/prs?scenario=head-moved").then(() => analyze(p))],
  ["home-in-progress", (p) => p.goto("/prs?scenario=existing-running").then(() => analyze(p))],
  ["home-paused", (p) => p.goto("/prs?scenario=existing-paused").then(() => analyze(p))],
  ["home-failed", (p) => p.goto("/prs?scenario=existing-failed").then(() => analyze(p))],
  ["home-no-app", (p) => p.goto("/prs?scenario=no-app").then(() => analyze(p))],
  [
    "home-cancelled",
    (p) =>
      p
        .goto(`${progress}?scenario=running`)
        .then(() => p.getByRole("button", { name: "Cancel analysis" }).click())
        .then(() => p.getByRole("button", { name: "Confirm cancel", exact: true }).click()),
  ],
  ["progress-planning", (p) => p.goto(`${progress}?scenario=planning`)],
  ["progress-running", (p) => p.goto(`${progress}?scenario=running`)],
  ["progress-paused", (p) => p.goto(`${progress}?scenario=paused`)],
  ["progress-failed", (p) => p.goto(`${progress}?scenario=failed`)],
  ["progress-failed-early", (p) => p.goto(`${progress}?scenario=failed-early`)],
  ["progress-finished", (p) => p.goto(`${progress}?scenario=finished`)],
  ["progress-unknown", (p) => p.goto("/analyses/00000000-0000-0000-0000-000000000000/progress")],
  ["settings", (p) => p.goto("/settings")],
  ["settings-llm-error", (p) => p.goto("/settings/llm?scenario=llm-error")],
  ["settings-llm-unknown", (p) => p.goto("/settings/llm?scenario=llm-unknown")],
  ["settings-no-app", (p) => p.goto("/settings/github-sync?scenario=no-app")],
  [
    "settings-installations-not-checked",
    (p) => p.goto("/settings/github-sync?scenario=installations-not-checked"),
  ],
  [
    "settings-repo-not-known",
    (p) => p.goto("/settings/repositories?scenario=repo-not-known&repo=acme%2Fnode-client"),
  ],
];

for (const colorScheme of ["light", "dark"] as const) {
  for (const [width, height] of [
    [1440, 900],
    [3440, 1440],
  ] as const) {
    test(`screens ${colorScheme} ${width}x${height}`, async ({ browser }) => {
      test.skip(!dir, "SCREENSHOT_DIR not set");
      test.setTimeout(120_000);
      const page = await browser.newPage({ colorScheme, viewport: { width, height } });
      for (const [name, reach] of STATES) {
        await reach(page);
        await page.getByRole("main").waitFor();
        await expect(page.getByText(/^(Loading|Connecting)/)).toHaveCount(0);
        await page.waitForTimeout(400);
        await page.screenshot({ path: `${dir}/${name}-${colorScheme}-${width}.png` });
      }
      await page.close();
    });
  }
}
