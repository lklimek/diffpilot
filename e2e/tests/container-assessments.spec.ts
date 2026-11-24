import { PR_PAGE_PATH, signIn, sql } from "../support/app";
import { expect, test } from "../support/fixtures";

test.use({ reviewLoop: true });

test("the fake container runtime returns accepted results for both frozen assessment methods", async ({
  page,
  stack,
}) => {
  test.skip(!stack.stateDir, "needs an isolated fake stack");
  await signIn(page);
  const me = await (await page.request.get("/api/v1/me")).json();
  const response = await page.request.put("/api/v1/settings/comment-assessment", {
    headers: { "X-CSRF-Token": me.csrf_token, Origin: new URL(page.url()).origin },
    data: { primary_method: "claudius_host", ab_prs: ["acme/node-client#482"], ab_until: null },
  });
  expect(response.ok()).toBeTruthy();
  await page.goto(PR_PAGE_PATH);
  await expect
    .poll(
      () =>
        sql(stack, "SELECT COUNT(DISTINCT method) FROM assessment_method_runs WHERE status = 'succeeded'"),
      { timeout: 60_000 },
    )
    .toEqual(["2"]);
  expect(
    sql(
      stack,
      `SELECT COUNT(*) FROM assessment_method_runs r JOIN jobs j ON j.id = r.job_id
       WHERE r.status = 'succeeded' AND json_extract(j.result, '$.assessment.method') != r.method`,
    ),
  ).toEqual(["0"]);
});
