import {
  analyseAllRemainingFlows,
  analysisState,
  analyze,
  checkScreen,
  heading,
  signIn,
  waitForState,
} from "../support/app";
import { logLeaks, stackSecrets } from "../support/secrets";
import { expect, test } from "../support/fixtures";
import { screens } from "../support/screens";

test.describe.configure({ mode: "serial" });

test("sign in, analyse the fixture PR, open flow 1 step 1", async ({ page, context, stack }) => {
  await page.goto("/");
  await expect(page.getByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
  await checkScreen(page);
  await screens(page, "sign-in");
  await signIn(page);
  await checkScreen(page);
  const id = await analyze(page);
  await checkScreen(page);
  await screens(page, "progress");

  // Flows run on request only: after the plan and the PR context nothing is analysed until asked.
  await expect(
    page.getByText(
      "The plan is ready. Nothing is being analysed. Choose a flow to analyse, or analyse all remaining flows.",
    ),
  ).toBeVisible({ timeout: 180_000 });
  await expect(page.getByText(/^0 of 5 flows analysed/)).toBeVisible();
  await page.waitForTimeout(2_000);
  expect(await analysisState(page, id)).toMatchObject({ state: "running", done: 0, idle: true });
  expect((await analysisState(page, id)).stage).toBeUndefined();
  await analyseAllRemainingFlows(page);
  await waitForState(page, id, "finished");
  await checkScreen(page);
  const openReview = page.getByRole("link", { name: "Open review" });
  await expect(openReview).toBeVisible();
  await openReview.click();
  await expect(page).toHaveURL(new RegExp(`/analyses/${id}$`));
  await expect(heading(page)).toHaveText("PR context: acme/node-client #482");
  await checkScreen(page);
  await screens(page, "pr-context");

  await page.keyboard.press("j");
  await expect(heading(page)).toHaveText("1 Send an RPC request with retry");
  await checkScreen(page);
  await screens(page, "flow-intro");

  await page.keyboard.press("j");
  await expect(heading(page)).toHaveText("1 Trigger: callers of send");
  const code = page.getByRole("region", { name: "src/mempool/mod.rs, lines 126 to 132" });
  await expect(code).toContainText("pub async fn broadcast_tx(client: &RpcClient, tx: &Transaction)");
  await expect(code).toContainText("let resp = client.send(req).await?;");
  await checkScreen(page);
  await screens(page, "step1");

  const me = (await (await page.request.get("/api/v1/me")).json()) as { csrf_token: string };
  stack.secrets.push(
    ["CSRF token", me.csrf_token],
    ...(await context.cookies()).map((cookie): [string, string] => [`cookie ${cookie.name}`, cookie.value]),
  );
});

test("every agent job ran with its own read-only installation token, revoked afterwards", async ({
  stack,
}) => {
  test.skip(!stack.githubCredentials, "fake GitHub's record is out of reach");
  // plan, PR context, one per flow: each job's token comes from fake GitHub, which grants only a read-only scope
  // for the PR repository to a JWT signed with the App key.
  let issued = await stack.githubCredentials?.();
  expect(issued?.installation_tokens.length ?? 0).toBeGreaterThanOrEqual(3);
  expect(new Set(issued?.installation_tokens).size).toBe(issued?.installation_tokens.length);
  await expect
    .poll(
      async () => {
        issued = await stack.githubCredentials?.();
        return [...(issued?.revoked ?? [])].sort();
      },
      { timeout: 30_000 },
    )
    .toEqual([...(issued?.installation_tokens ?? [])].sort());
});

test("component logs contain no secret or token so far", async ({ stack }) => {
  test.skip(
    !stack.stateDir || !stack.githubCredentials,
    "the logs of a stack started elsewhere are out of reach",
  );
  expect(stack.secrets.length, "the scenario registered the session secrets").toBeGreaterThan(1);
  const issued = await stack.githubCredentials?.();
  expect(issued?.jwts.length, "fake GitHub accepted App JWTs").toBeGreaterThan(0);
  expect(
    logLeaks(
      stack,
      stackSecrets(stack.stateDir ?? "", issued ?? { jwts: [], installation_tokens: [], revoked: [] }),
    ),
  ).toEqual([]);
});
