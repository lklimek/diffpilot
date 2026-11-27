import { randomUUID } from "node:crypto";
import { checkScreen, PR_PAGE_PATH, signIn, sql } from "../support/app";
import { api } from "../support/dashboard";
import { expect, test } from "../support/fixtures";

test.use({ reviewLoop: true });

test("an agent submits a finding and annotates a GitHub comment before the owner opens the PR", async ({
  page,
  stack,
}) => {
  test.skip(!stack.stateDir, "needs a fresh isolated stack");
  await signIn(page);
  expect(sql(stack, "SELECT COUNT(*) FROM comment_prs")).toEqual(["0"]);

  const token = await api(page, "POST", "/api/v1/settings/access-tokens", {
    name: "Local review agent",
    scopes: ["read", "review_loop_submit"],
    expires_in_days: 30,
  });
  expect(token.status).toBe(201);
  const secret = String(token.body?.token);
  stack.secrets.push(["personal access token (submit)", secret]);

  const standalone = {
    submission_key: randomUUID(),
    body: "The retry backoff may exceed its maximum after the final attempt.",
    target: { kind: "standalone" },
  };
  const annotate = {
    submission_key: randomUUID(),
    body: "This finding also applies to the retry watchdog.",
    target: {
      kind: "github",
      comment: "https://github.com/acme/node-client/pull/482#discussion_r3000000201",
    },
  };
  const submit = (data: typeof standalone | typeof annotate) =>
    fetch(`${stack.url}/api/v1/prs/acme/node-client/482/comments/submissions`, {
      method: "POST",
      headers: { authorization: `Bearer ${secret}`, "content-type": "application/json" },
      body: JSON.stringify(data),
    });
  const responses = await Promise.all([submit(standalone), submit(standalone)]);
  expect(responses.map((response) => response.status).sort()).toEqual([200, 201]);
  const [entry, replay] = (await Promise.all(responses.map((response) => response.json()))) as {
    submission_id: string;
    item_no: number | null;
    url: string;
  }[];
  expect(replay).toEqual(entry);
  expect(entry.item_no).toEqual(expect.any(Number));
  const thread = await submit(annotate);
  expect(thread.status, await thread.clone().text()).toBe(201);
  const attachment = (await thread.json()) as typeof entry;
  expect(attachment.item_no).not.toBe(entry.item_no);
  expect(sql(stack, "SELECT COUNT(*) FROM comment_effects")).toEqual(["0"]);

  const entryUrl = new URL(entry.url);
  const attachmentUrl = new URL(attachment.url);
  expect(entryUrl.pathname).toBe(`${PR_PAGE_PATH}/comments`);
  expect(attachmentUrl.pathname).toBe(entryUrl.pathname);
  await page.goto(entryUrl.pathname + entryUrl.search + entryUrl.hash);
  const finding = page.getByRole("article").filter({
    has: page.getByRole("heading", { name: `C${entry.item_no} · ${standalone.body}` }),
  });
  await expect(finding.getByText(standalone.body, { exact: true })).toBeVisible();
  await expect(finding.getByText("API", { exact: true })).toBeVisible();
  await expect(finding).toContainText("Local review agent · agent");
  await checkScreen(page);

  await page.goto(attachmentUrl.pathname + attachmentUrl.search + attachmentUrl.hash);
  await expect(page.getByText(annotate.body, { exact: true })).toBeVisible();
  await expect(page.getByText(/GitHub \+ agent/).first()).toBeVisible();
  await expect(page.getByText(/Cap the backoff delay/).first()).toBeVisible();
  await checkScreen(page);
});
