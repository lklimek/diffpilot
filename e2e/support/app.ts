// Steps and checks shared by the journeys.
import AxeBuilder from "@axe-core/playwright";
import { expect, type Page } from "@playwright/test";
import type { Stack } from "./stack";

/** The fixture PR (fixtures/pr-482), the only one fake GitHub serves. */
export const PR_URL = "https://github.com/acme/node-client/pull/482";
/** One analysis of the fixture PR, the runner's 5 s claim interval included. */
export const FINISH_TIMEOUT_MS = 90_000;

/** The heading of the block shown in the centre column. */
export const heading = (page: Page) => page.locator("main h2:not([data-column-title])").first();
export const nav = (page: Page) => page.getByRole("navigation", { name: "Flows and steps" });
export const inspector = (page: Page) => page.getByRole("complementary", { name: "Inspector" });

/** Signs in as the fake user `alice` through fake GitHub and waits for Home. */
export async function signIn(page: Page): Promise<void> {
  await page.goto("/");
  await page.getByRole("link", { name: "Sign in with GitHub" }).click();
  await expect(page.getByLabel("GitHub pull request URL")).toBeVisible();
  await expect(page.getByRole("banner")).toContainText("alice");
}

/** Pastes the PR URL on Home, presses Analyze and returns the new analysis id (progress screen open). */
export async function analyze(page: Page): Promise<string> {
  await page.getByLabel("GitHub pull request URL").fill(PR_URL);
  const created = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" && new URL(response.url()).pathname === "/api/v1/analyses",
  );
  await page.getByRole("button", { name: "Analyze" }).click();
  const response = await created;
  expect(response.status(), await response.text()).toBe(201);
  const { id } = (await response.json()) as { id: string };
  await expect(page).toHaveURL(new RegExp(`/analyses/${id}/progress$`));
  return id;
}

/** The analysis state as the API reports it. */
export async function analysisState(
  page: Page,
  id: string,
): Promise<{ state: string } & Record<string, unknown>> {
  const response = await page.request.get(`/api/v1/analyses/${id}`);
  expect(response.ok(), await response.text()).toBe(true);
  return ((await response.json()) as { state: { state: string } }).state;
}

/** Polls the analysis until its state is `wanted`; fails with the last state seen. */
export async function waitForState(page: Page, id: string, wanted: string, timeout = FINISH_TIMEOUT_MS) {
  const deadline = Date.now() + timeout;
  let last = "none";
  while (Date.now() < deadline) {
    const state = await analysisState(page, id);
    last = JSON.stringify(state);
    if (state.state === wanted) return;
    await page.waitForTimeout(500);
  }
  throw new Error(`analysis ${id} did not reach ${wanted} within ${timeout / 1000} s; last state: ${last}`);
}

/**
 * Signs in and opens the review of the PR at PR context: analyses it to the end the first time, resumes the
 * existing analysis of the stack afterwards. Returns the analysis id.
 */
export async function finishedReview(page: Page): Promise<string> {
  await signIn(page);
  await page.getByLabel("GitHub pull request URL").fill(PR_URL);
  const created = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" && new URL(response.url()).pathname === "/api/v1/analyses",
  );
  await page.getByRole("button", { name: "Analyze" }).click();
  const resume = page.getByRole("link", { name: "Resume review" });
  // The loser of the race stays pending until the page closes; its rejection is expected.
  const existing = await Promise.race([
    created.then(() => false).catch(() => false),
    resume
      .waitFor()
      .then(() => true)
      .catch(() => false),
  ]);
  if (existing) {
    await resume.click();
  } else {
    const response = await created;
    expect(response.status(), await response.text()).toBe(201);
    const { id } = (await response.json()) as { id: string };
    await waitForState(page, id, "finished");
    await page.getByRole("link", { name: "Open review" }).click();
  }
  await expect(page).toHaveURL(/\/analyses\/[0-9a-f-]+$/);
  await navTo(page, /^PR context$/);
  await expect(heading(page)).toHaveText("PR context: acme/node-client #482");
  return new URL(page.url()).pathname.split("/").at(-1) ?? "";
}

/**
 * Clicks a navigation entry. The pointer first leaves the entry clicked before: a focused entry keeps its tooltip,
 * which may cover the next entry until the pointer moves away.
 */
export async function navTo(page: Page, name: string | RegExp): Promise<void> {
  await page.mouse.move(0, 0);
  await nav(page).getByRole("button", { name }).click();
}

/** Opens step `step` of flow `flow` from the navigation. */
export async function openStep(page: Page, flow: number, step: number): Promise<void> {
  await navTo(page, new RegExp(`^Flow ${flow}:`));
  await navTo(page, new RegExp(`^Step ${step}:`));
}

const BANNED = [
  /\bno\s+risks?\b/i,
  /\bnothing\s+to\s+review\b/i,
  /\bno\s+review\s+needed\b/i,
  /\bnoise\b/i,
  /\bverified\b/i,
  /\bwas\s+wrong\b/i,
  /\bconfidence\b/i,
  /\d\s?%/,
];

/** Brief §1 / plan §12.4 on the rendered page text (tooltips included): no banned phrase, no percentage. */
export async function expectTrustWording(page: Page): Promise<void> {
  const text = await page.locator("body").evaluate((body) => body.textContent ?? "");
  for (const pattern of BANNED) expect(text, `banned wording ${pattern}`).not.toMatch(pattern);
}

/** No serious or critical axe violation (WCAG 2.2 AA tags). */
export async function expectAccessible(page: Page): Promise<void> {
  const results = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"])
    .analyze();
  // Line numbers keep the code's line height: the WCAG 2.5.8 exception for targets constrained by the line
  // height of non-target text (web/tests/a11y.spec.ts).
  const lineNumber = (id: string, html: string) => id === "target-size" && html.includes("data-line-action");
  const serious = results.violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => ({ ...v, nodes: v.nodes.filter((n) => !lineNumber(v.id, n.html)) }))
    .filter((v) => v.nodes.length > 0);
  expect(serious.map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(" ")}`)).toEqual([]);
}

/** Wording rules and the accessibility pass on the current screen state. */
export async function checkScreen(page: Page): Promise<void> {
  await expectTrustWording(page);
  await expectAccessible(page);
}

/** Fake MemCan's view (tools/fake-memcan `GET /_fake/state`). */
export interface MemcanState {
  mode: { mode: string };
  memories: Record<string, Record<string, unknown>>;
  deleted: string[];
  calls: { tool: string; arguments: Record<string, unknown> }[];
}

/** Reads or switches the fake MemCan of `stack`. */
export function memcan(page: Page, stack: Stack) {
  if (!stack.memcan) throw new Error("this stack has no reachable fake MemCan (SR_E2E_FAKE_MEMCAN_URL)");
  const { url, token } = stack.memcan;
  const headers = { authorization: `Bearer ${token}` };
  return {
    state: async (): Promise<MemcanState> => {
      const response = await page.request.get(`${url}/_fake/state`, { headers });
      expect(response.ok()).toBe(true);
      return (await response.json()) as MemcanState;
    },
    mode: async (mode: Record<string, unknown>) => {
      const response = await page.request.post(`${url}/_fake/mode`, { headers, data: mode });
      expect(response.status()).toBe(204);
    },
  };
}
