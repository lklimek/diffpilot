// Journey of the review loop (docs/review-workflow-plan.md §3, v1.3): the PR page's "Process PR comments" from fake
// GitHub's review-loop reads (fixtures/review-loop/github), Comments, the fake provider's verify_comments
// assessment, a decision whose previewed effects are the stored ones, the decision log; an implementing agent's
// execute token claims the decision (RF3) and reports an effect (RF4), and the log shows the report.
import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { Page } from "@playwright/test";
import { checkScreen, PR_PAGE_PATH, signIn } from "../support/app";
import { expect, test } from "../support/fixtures";

test.use({ reviewLoop: true });
test.describe.configure({ mode: "serial" });

const API = "/api/v1/prs/acme/node-client/482";
const COMMENTS_PATH = `${PR_PAGE_PATH}/comments`;
/** The first question of the fixture's queue: thepastaclaw's blocking inline thread of round 1. */
const ITEM = "C3";
const ITEM_TITLE = "Count only finished attempts in the metric";
const CAPABILITIES = ["fix", "reply", "resolve"];

interface PlannedEffect {
  seq: number;
  after: number | null;
  kind: string;
  executor: string;
  params: unknown;
}
interface StoredEffect extends PlannedEffect {
  id: string;
  state: string;
}
interface StoredDecision {
  id: string;
  item_no: number;
  kind: string;
  comment: string | null;
  effects: StoredEffect[];
}

const planned = ({ seq, after, kind, executor, params }: PlannedEffect): PlannedEffect => ({
  seq,
  after,
  kind,
  executor,
  params,
});

/** The decision on the item in the Decision log page's "Decisions". */
const loggedDecision = (page: Page) =>
  page
    .getByRole("region", { name: "Decisions", exact: true })
    .getByRole("listitem")
    .filter({ hasText: `· ${ITEM} · ` })
    .first();

/** The PR's decision log (RC10). */
async function decisionLog(page: Page): Promise<StoredDecision[]> {
  const response = await page.request.get(`${API}/comments/log`);
  expect(response.ok(), await response.text()).toBe(true);
  return ((await response.json()) as { decisions: StoredDecision[] }).decisions;
}

test("PR page → Comments → the LLM assessment arrives → a decision stores the previewed effects; the log lists it", async ({
  page,
  stack,
}) => {
  test.skip(
    !stack.script || !stack.stateDir,
    "needs the fake provider's script and the runner log (a local stack)",
  );
  // Slow enough to see the assessment running (4 recorded lines of the fake provider's neutral answer).
  stack.script?.([{ id: "verify", job: "verify_comments", action: { replay: { step_delay_ms: 3000 } } }]);
  await signIn(page);
  // TODO(W4): the legacy fake executor must echo JobSpec.assessment to satisfy the frozen-method fence.
  // This journey uses the retained prompt-v4 recording of the scripted fake provider.
  const settingsRoute = "/api/v1/settings/comment-assessment";
  const settings = await (await page.request.get(settingsRoute)).json();
  const me = await (await page.request.get("/api/v1/me")).json();
  const configured = await page.request.put(settingsRoute, {
    headers: { "X-CSRF-Token": me.csrf_token },
    data: { revision: settings.revision, config: { ...settings.config, primary_method: "prompt_v4" } },
  });
  expect(configured.ok(), await configured.text()).toBe(true);
  // The effects the card previews (RC16), as the server planned them.
  const previews: Promise<{ item_no: number; effects: PlannedEffect[] }[]>[] = [];
  page.on("response", (response) => {
    if (response.request().method() === "POST" && response.url().endsWith(`${API}/comments/effects-preview`))
      previews.push(response.json());
  });

  // Opening the PR page reads the PR's threads, reviews and comments from GitHub and queues the assessment.
  await page.goto(PR_PAGE_PATH);
  const card = page.getByRole("region", { name: "Process PR comments" });
  await expect(card.getByRole("term")).toHaveText([
    "Unresolved threads on GitHub",
    "Questions for you",
    "Waiting for agents",
    "LLM assessment",
    "Round",
  ]);
  // D3 includes three extra prose sources; gate and CI status comments are ignored.
  await expect(card.getByRole("definition")).toHaveText([
    /^6 read \d\d:\d\d/,
    "13",
    "0 decisions",
    "LLM assessment running: 0 of 12 comments",
    "3 (thepastaclaw)",
  ]);
  await expect(page.getByRole("region", { name: "Review a PR" })).toContainText("No analysis yet.");
  await checkScreen(page);

  await card.getByRole("link", { name: "Open Comments →" }).click();
  await expect(page).toHaveURL(new RegExp(`${COMMENTS_PATH}$`));
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Comments of acme/node-client #482");
  const question = page.getByRole("article", { name: `${ITEM} · ${ITEM_TITLE}` });
  await expect(question).toBeVisible();
  await checkScreen(page);

  // The assessment lands on the open question: the recommendation and the queue's mark.
  await question.getByRole("button", { name: "LLM assessment", exact: true }).click();
  await expect(question).toContainText("Recommendation: Fix here — Fake recommendation for tests.", {
    timeout: 60_000,
  });
  const log = readFileSync(join(stack.stateDir ?? "", "logs", "runner.log"), "utf8");
  expect(log).toMatch(/job claimed job=\S+ attempt=\d+ kind="verify_comments"/);
  expect(log).toMatch(/fake provider script rule applied job=\S+ rule=verify/);
  await expect(question).toContainText("thepastaclaw: Blocking");
  await expect(question.getByRole("region", { name: "LLM assessment" })).toContainText(
    "partly valid at the PR head",
  );
  const queue = page.getByRole("navigation", { name: "Queue" });
  await expect(
    queue.getByRole("button", { name: new RegExp(`^${ITEM} ${ITEM_TITLE} .*LLM recommends Fix here$`) }),
  ).toBeVisible();
  await checkScreen(page);

  // Decide with the recommended choice: what "What happens next" previews is what the server stores.
  const form = question.getByRole("form", { name: `Decision on ${ITEM}` });
  await expect(form.getByRole("radio", { name: "Fix here (Recommended)" })).toBeChecked();
  await form.getByRole("textbox", { name: "Your comment (optional)" }).fill("Keep the test small.");
  const next = form.getByRole("region", { name: "What happens next" });
  const lines = next.getByRole("list").first().getByRole("listitem");
  await expect(lines).toHaveText([
    /fixes it and pushes\.$/,
    /replies with the reply text below\.$/,
    /resolves the thread\.$/,
    "The executing agent receives your comment with the decision; it is not posted.",
  ]);
  await expect(next.getByRole("list").first()).not.toHaveAttribute("aria-busy", "true");
  await checkScreen(page);
  expect(previews.length, "the effects preview was asked").toBeGreaterThan(0);
  const plans = await Promise.all(previews);
  const shown =
    plans
      .flat()
      .filter((row) => row.item_no === 3)
      .at(-1)?.effects ?? [];
  expect(shown.map((effect) => effect.kind)).toEqual(["fix", "reply", "resolve"]);

  const decided = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      response.url().endsWith(`${API}/comments/items/${ITEM}/decisions`),
  );
  await form.getByRole("button", { name: "Decide" }).click();
  expect((await decided).status()).toBe(201);
  await expect(page.getByRole("article", { name: `${ITEM} · ${ITEM_TITLE}` })).toHaveCount(0);
  await checkScreen(page);

  const [stored] = await decisionLog(page);
  expect(stored).toMatchObject({ item_no: 3, kind: "fix_here", comment: "Keep the test small." });
  expect(stored.effects.map(planned), "the stored effects are the previewed ones").toEqual(
    shown.map(planned),
  );

  await page
    .getByRole("navigation", { name: "Comments pages" })
    .getByRole("link", { name: "Decision log and agents" })
    .click();
  await expect(page).toHaveURL(new RegExp(`${COMMENTS_PATH}/log$`));
  const entry = loggedDecision(page);
  await expect(entry).toContainText(`· ${ITEM} · Fix here`);
  await expect(entry).toContainText('Your comment (sent to the agent, not posted): "Keep the test small."');
  await expect(entry.getByRole("listitem")).toHaveText([/^fix: open/, /^reply: /, /^resolve: /]);
  await checkScreen(page);
});

test("an execute token claims the decision (RF3) and reports its fix done (RF4); the decision log shows it", async ({
  page,
  stack,
}) => {
  test.skip(!stack.script || !stack.stateDir, "needs the first test's decision (a local stack)");
  await signIn(page);
  await page.goto("/settings");
  const tokens = page.getByRole("region", { name: "Access tokens" });
  await tokens.getByRole("button", { name: "New token" }).click();
  const dialog = page.getByRole("dialog", { name: "New access token" });
  await dialog.getByLabel("Name").fill("e2e agent");
  await dialog.getByRole("checkbox", { name: /^Also review_loop_execute/ }).check();
  await expect(dialog).toContainText("Scope: read and review_loop_execute.");
  await dialog.getByRole("button", { name: "Create token" }).click();
  const created = page.getByRole("dialog", { name: "Token created" });
  const secret = await created.getByLabel("Your new token").inputValue();
  stack.secrets.push(["personal access token (execute)", secret]);
  await checkScreen(page);
  await created.getByRole("button", { name: "Done" }).click();

  // The agent's side, with the token alone: no cookie, no CSRF token.
  const agent = (path: string, body?: unknown) =>
    fetch(`${stack.url}${path}`, {
      method: body === undefined ? "GET" : "POST",
      headers: {
        authorization: `Bearer ${secret}`,
        ...(body === undefined ? {} : { "content-type": "application/json" }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  const [decision] = await decisionLog(page);
  const feed = await agent(`/api/v1/decision-feed?capabilities=${CAPABILITIES.join(",")}`);
  expect(feed.status).toBe(200);
  const offered = ((await feed.json()) as { decisions: { decision: StoredDecision }[] }).decisions;
  expect(offered.map((d) => d.decision.id)).toEqual([decision.id]);

  const claim = await agent(`/api/v1/decision-feed/decisions/${decision.id}/claim`, {
    capabilities: CAPABILITIES,
  });
  expect(claim.status).toBe(201);
  const claimed = (await claim.json()) as { decision: { decision: StoredDecision }; lease_until: string };
  const fix = claimed.decision.decision.effects.find((effect) => effect.seq === 1);
  expect(fix?.kind).toBe("fix");
  const head = ((await (await page.request.get(`${API}/comments`)).json()) as { head: string }).head;
  const report = await agent(`/api/v1/decision-feed/effects/${fix?.id}/reports`, {
    outcome: "done",
    reference: head,
    message: null,
    idempotency_key: randomUUID(),
  });
  expect(report.status, await report.text()).toBe(201);

  await page.goto(`${COMMENTS_PATH}/log`);
  const effects = loggedDecision(page).getByRole("listitem");
  await expect(effects.nth(0)).toContainText("fix: done · Agent e2e agent reports: done");
  await expect(
    effects.nth(0).getByRole("link", { name: `Open commit ${head.slice(0, 7)} on GitHub` }),
  ).toBeVisible();
  await expect(effects.nth(1)).toContainText(/^reply: .* by e2e agent \(lease until (?:[^)]+, )?\d\d:\d\d\)/);
  await checkScreen(page);
});
