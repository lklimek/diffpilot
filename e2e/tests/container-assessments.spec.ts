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
  const settingsRoute = "/api/v1/settings/comment-assessment";
  const settings = await (await page.request.get(settingsRoute)).json();
  const response = await page.request.put(settingsRoute, {
    headers: { "X-CSRF-Token": me.csrf_token, Origin: new URL(page.url()).origin },
    data: {
      revision: settings.revision,
      config: { ...settings.config, primary_method: "claudius_host", ab_prs: ["acme/node-client#482"] },
    },
  });
  expect(response.ok(), await response.text()).toBeTruthy();
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
    WHERE r.status = 'succeeded'`,
    ),
  ).toEqual(["2"]);
  expect(
    sql(
      stack,
      `SELECT COUNT(*) FROM assessment_method_runs r JOIN jobs j ON j.id = r.job_id
       WHERE r.status = 'succeeded' AND json_extract(j.result, '$.assessment.method') IS NOT r.method`,
    ),
  ).toEqual(["0"]);
  expect(
    sql(
      stack,
      `SELECT COUNT(DISTINCT json_extract(source, '$.source_kind'))
    FROM assessment_comparison_items WHERE method = 'claudius_host'
    AND json_extract(source, '$.source_kind') IN ('review_body', 'pr_comment')
    AND length(json_extract(source, '$.quote')) >= 12`,
    ),
  ).toEqual(["2"]);
  expect(
    sql(
      stack,
      `SELECT COUNT(*) FROM assessment_comparison_items d
    JOIN assessment_comparison_items c ON c.pass_id = d.pass_id AND c.method = d.method
      AND c.comparison_key = d.duplicate_of
    WHERE d.method = 'claudius_host' AND d.status = 'duplicate'
      AND json_extract(d.source, '$.source_kind') = 'pr_comment'
      AND json_extract(c.source, '$.source_kind') = 'review_body'`,
    ),
  ).toEqual(["1"]);
});
