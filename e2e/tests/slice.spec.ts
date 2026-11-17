import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { type Page } from "@playwright/test";
import { analyze, heading, signIn, waitForState } from "../support/app";
import { expect, test } from "../support/fixtures";

// Light and dark at both widths when set, e.g. SCREENSHOT_DIR=/data/artifacts/smart-review/<date>/wp2-e2e.
const SCREENSHOT_DIR = process.env.SCREENSHOT_DIR;
const VIEWS = [
  { width: 1440, height: 900 },
  { width: 3440, height: 1440 },
] as const;
// Credentials of the fake GitHub recordings (fixtures/pr-482/github/oauth/access-token.json,
// rest/installation-token.json).
const FIXTURE_SECRETS: [string, string][] = [
  ["OAuth code", "fixture-oauth-code"],
  ["user access token", "fixture-user-token-alice"],
  ["user refresh token", "fixture-refresh-token-alice"],
  ["installation token", "fixture-installation-token-482"],
];
// The secret values in the launcher's stack.env; the rest (URLs, paths, ids) is not secret.
const SECRET_KEYS = ["SR_SECRET_KEY", "SR_RUNNER_SECRET", "SR_GITHUB_CLIENT_SECRET", "SR_MEMCAN_TOKEN"];

test.describe.configure({ mode: "serial" });

// Session cookie and CSRF token of the scenario, for the log scan.
const sessionSecrets: [string, string][] = [];

async function shoot(page: Page, name: string): Promise<void> {
  if (!SCREENSHOT_DIR) return;
  for (const colorScheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme });
    for (const view of VIEWS) {
      await page.setViewportSize(view);
      await page.screenshot({ path: join(SCREENSHOT_DIR, `${name}-${colorScheme}-${view.width}.png`) });
    }
  }
  await page.emulateMedia({ colorScheme: "light" });
  await page.setViewportSize(VIEWS[0]);
}

test("sign in, analyse the fixture PR, open flow 1 step 1", async ({ page, context }) => {
  await page.goto("/");
  await expect(page.getByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
  await shoot(page, "sign-in");
  await signIn(page);
  const id = await analyze(page);
  await shoot(page, "progress");

  await waitForState(page, id, "finished");
  const openReview = page.getByRole("link", { name: "Open review" });
  await expect(openReview).toBeVisible();
  await openReview.click();
  await expect(page).toHaveURL(new RegExp(`/analyses/${id}$`));
  await expect(heading(page)).toHaveText("PR context: acme/node-client #482");
  await shoot(page, "pr-context");

  await page.keyboard.press("j");
  await expect(heading(page)).toHaveText("1 Send an RPC request with retry");
  await shoot(page, "flow-intro");

  await page.keyboard.press("j");
  await expect(heading(page)).toHaveText("1 Trigger: callers of send");
  const code = page.getByRole("region", { name: "src/mempool/mod.rs, lines 126 to 132" });
  await expect(code).toContainText("pub async fn broadcast_tx(client: &RpcClient, tx: &Transaction)");
  await expect(code).toContainText("let resp = client.send(req).await?;");
  await shoot(page, "step1");

  const me = (await (await page.request.get("/api/v1/me")).json()) as { csrf_token: string };
  sessionSecrets.push(
    ["CSRF token", me.csrf_token],
    ...(await context.cookies()).map((cookie): [string, string] => [`cookie ${cookie.name}`, cookie.value]),
  );
});

test("component logs contain no secret or token", ({ stack }) => {
  test.skip(!stack.stateDir, "the logs of a stack started elsewhere are out of reach");
  const stateDir = stack.stateDir ?? "";
  const env = readFileSync(join(stateDir, "stack.env"), "utf8");
  const generated = env
    .split("\n")
    .map((line): [string, string] => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)])
    .filter(([key]) => SECRET_KEYS.includes(key));
  expect(new Set(generated.map(([key]) => key))).toEqual(new Set(SECRET_KEYS));
  expect(sessionSecrets.length, "the scenario recorded the session secrets").toBeGreaterThan(1);
  const secrets = [...generated, ...FIXTURE_SECRETS, ...sessionSecrets].filter(
    ([, value]) => value.length >= 8,
  );

  const logDir = join(stateDir, "logs");
  const logs = readdirSync(logDir);
  expect(logs).toEqual(
    expect.arrayContaining(["server.log", "runner.log", "fake-github.log", "fake-memcan.log"]),
  );
  for (const log of logs) {
    const text = readFileSync(join(logDir, log), "utf8");
    expect(text.length, `${log} is empty`).toBeGreaterThan(0);
    for (const [name, value] of secrets) {
      expect(text.includes(value), `${log} contains the value of ${name}`).toBe(false);
    }
  }
});
