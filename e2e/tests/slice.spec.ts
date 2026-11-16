import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test, type Page } from "@playwright/test";

const PR_URL = "https://github.com/acme/node-client/pull/482";
const FINISH_TIMEOUT_MS = 60_000;
// Light and dark at both widths when set, e.g. SCREENSHOT_DIR=/data/artifacts/smart-review/<date>/wp2-e2e.
const SCREENSHOT_DIR = process.env.SCREENSHOT_DIR;
const VIEWS = [
  { width: 1440, height: 900 },
  { width: 3440, height: 1440 },
] as const;
// Credentials of the fake GitHub recording (fixtures/pr-482/github/oauth/access-token.json).
const FIXTURE_SECRETS: [string, string][] = [
  ["OAuth code", "fixture-oauth-code"],
  ["user access token", "fixture-user-token-alice"],
  ["user refresh token", "fixture-refresh-token-alice"],
];
// The secret values in the launcher's stack.env; the rest (URLs, paths, ids) is not secret.
const SECRET_KEYS = ["SR_SECRET_KEY", "SR_RUNNER_SECRET", "SR_GITHUB_CLIENT_SECRET", "SR_MEMCAN_TOKEN"];

test.describe.configure({ mode: "serial" });

const stateDir = () => process.env.SR_E2E_STATE_DIR ?? "";
const heading = (page: Page) => page.getByRole("main").getByRole("heading", { level: 2 }).first();
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

/** Polls the analysis until it is finished; fails with the last state seen. */
async function waitUntilFinished(page: Page, id: string): Promise<void> {
  const deadline = Date.now() + FINISH_TIMEOUT_MS;
  let last = "none";
  while (Date.now() < deadline) {
    const response = await page.request.get(`/api/v1/analyses/${id}`);
    last = response.ok() ? JSON.stringify((await response.json()).state) : `HTTP ${response.status()}`;
    if (last.includes('"state":"finished"')) return;
    if (/"state":"(failed|cancelled)"/.test(last)) break;
    await page.waitForTimeout(500);
  }
  throw new Error(
    `analysis ${id} did not reach finished within ${FINISH_TIMEOUT_MS / 1000} s; last state: ${last}`,
  );
}

test("sign in, analyse the fixture PR, open flow 1 step 1", async ({ page, context }) => {
  await page.goto("/");
  const signIn = page.getByRole("link", { name: "Sign in with GitHub" });
  await expect(signIn).toBeVisible();
  await shoot(page, "sign-in");

  await signIn.click();
  const prUrl = page.getByLabel("GitHub pull request URL");
  await expect(prUrl).toBeVisible();
  await expect(page.getByRole("banner")).toContainText("alice");

  await prUrl.fill(PR_URL);
  const created = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" && new URL(response.url()).pathname === "/api/v1/analyses",
  );
  await page.getByRole("button", { name: "Analyze" }).click();
  const response = await created;
  expect(response.status(), await response.text()).toBe(201);
  const { id } = (await response.json()) as { id: string };
  await expect(page.getByText("The analysis has started.")).toBeVisible();
  await shoot(page, "progress");

  await waitUntilFinished(page, id);
  // The thin slice has no progress screen: submitting the URL again opens the finished analysis.
  await page.getByRole("button", { name: "Analyze" }).click();
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

test("component logs contain no secret or token", () => {
  const env = readFileSync(join(stateDir(), "stack.env"), "utf8");
  const generated = env
    .split("\n")
    .map((line): [string, string] => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)])
    .filter(([key]) => SECRET_KEYS.includes(key));
  expect(new Set(generated.map(([key]) => key))).toEqual(new Set(SECRET_KEYS));
  expect(sessionSecrets.length, "the scenario recorded the session secrets").toBeGreaterThan(1);
  const secrets = [...generated, ...FIXTURE_SECRETS, ...sessionSecrets].filter(
    ([, value]) => value.length >= 8,
  );

  const logDir = join(stateDir(), "logs");
  const logs = readdirSync(logDir);
  expect(logs).toEqual(expect.arrayContaining(["server.log", "runner.log", "fake-github.log"]));
  for (const log of logs) {
    const text = readFileSync(join(logDir, log), "utf8");
    expect(text.length, `${log} is empty`).toBeGreaterThan(0);
    for (const [name, value] of secrets) {
      expect(text.includes(value), `${log} contains the value of ${name}`).toBe(false);
    }
  }
});
