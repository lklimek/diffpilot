// Steps and checks shared by the journeys.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
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

/** The wording rules shared with scripts/check-wording.sh (brief §1, plan §12.4). */
const RULES = JSON.parse(
  readFileSync(new URL("../../scripts/wording-rules.json", import.meta.url), "utf8"),
) as { words: string; percent: string };
const WORDS = new RegExp(RULES.words, "iu");
const PERCENT = new RegExp(RULES.percent, "u");

/** Banned phrases and percentages in `text`, as `"match" in context`. */
export function wordingViolations(text: string, patterns: RegExp[] = [WORDS, PERCENT]): string[] {
  return patterns.flatMap((pattern) => {
    const match = new RegExp(pattern.source, `${pattern.flags}g`);
    return [...text.matchAll(match)].map((found) => {
      const at = found.index ?? 0;
      return `"${found[0]}" in "${text.slice(Math.max(0, at - 40), at + 40)}"`;
    });
  });
}

/**
 * Brief §1 / plan §12.4 on the rendered page: no banned phrase and no percentage in the page text (hidden tooltip
 * text included), and no `%` at all in the top bar.
 */
export async function expectTrustWording(page: Page): Promise<void> {
  // Text nodes joined by spaces (hidden text too; block boundaries keep word boundaries) and the rendered text
  // (inline markup inside a word).
  const text = await page.locator("body").evaluate((body) => {
    const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT, (node) =>
      node.parentElement?.closest("script, style") ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT,
    );
    const nodes: string[] = [];
    while (walker.nextNode()) nodes.push(walker.currentNode.textContent ?? "");
    return `${nodes.join(" ")}\n${(body as HTMLElement).innerText}`;
  });
  expect(wordingViolations(text), "banned wording on the page").toEqual([]);
  const banner = page.getByRole("banner");
  if ((await banner.count()) > 0) {
    const top = await banner
      .first()
      .evaluate((bar) => `${bar.textContent ?? ""} ${(bar as HTMLElement).innerText}`);
    expect(wordingViolations(top, [WORDS, /%/u]), "banned wording in the top bar").toEqual([]);
  }
}

/**
 * Focuses every rendered trigger of a tooltip (`aria-describedby` naming a `role="tooltip"` element), or the
 * focusable element inside it: the tip must show on keyboard focus and hide when focus leaves; returns the texts
 * shown and the problems found. No key is pressed (Escape is the app's global "close" shortcut).
 */
export async function sweepTooltips(page: Page): Promise<{ texts: string[]; problems: string[] }> {
  return page.evaluate(async () => {
    const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
    const settle = async () => {
      await frame();
      await frame();
    };
    const previous = document.activeElement as HTMLElement | null;
    const texts: string[] = [];
    const problems: string[] = [];
    const tipOf = (trigger: Element) =>
      (trigger.getAttribute("aria-describedby") ?? "")
        .split(/\s+/)
        .map((id) => document.getElementById(id))
        .find((element) => element?.getAttribute("role") === "tooltip");
    const focusable = "button, a[href], input, select, textarea, [tabindex]:not([tabindex='-1'])";
    for (const trigger of document.querySelectorAll<HTMLElement>("[aria-describedby]")) {
      const tip = tipOf(trigger);
      // Not rendered: hidden, display none, or the content of a closed <details>.
      if (!tip || !trigger.checkVisibility({ contentVisibilityAuto: true, visibilityProperty: true }))
        continue;
      const name = (trigger.getAttribute("aria-label") ?? trigger.textContent ?? "").trim().slice(0, 60);
      // A trigger that is not focusable itself opens through a focusable element inside it (focus bubbles).
      const target = trigger.matches(focusable) ? trigger : trigger.querySelector<HTMLElement>(focusable);
      // A disabled control takes no focus by design; its description stays in the accessibility tree.
      if (target?.matches(":disabled")) continue;
      if (!target) {
        problems.push(`tooltip trigger cannot take keyboard focus: "${name}"`);
        continue;
      }
      target.focus({ preventScroll: true });
      await settle();
      if (document.activeElement !== target) {
        // Re-rendered meanwhile (the node left the document): not a finding.
        if (!target.isConnected) continue;
        problems.push(`tooltip trigger cannot take keyboard focus: "${name}"`);
        continue;
      }
      if (tip.hidden) {
        problems.push(`tooltip not shown on focus: "${name}"`);
      } else {
        texts.push(tip.textContent ?? "");
      }
      target.blur();
      await settle();
      if (!tip.hidden) problems.push(`tooltip stays open after focus leaves "${name}"`);
    }
    previous?.focus?.({ preventScroll: true });
    await settle();
    return { texts, problems };
  });
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

/** Wording rules (page, top bar, every tooltip shown by keyboard) and the accessibility pass on the screen. */
export async function checkScreen(page: Page): Promise<void> {
  await expectTrustWording(page);
  const tooltips = await sweepTooltips(page);
  expect(tooltips.problems, "tooltips").toEqual([]);
  expect(wordingViolations(tooltips.texts.join("\n")), "banned wording in tooltips").toEqual([]);
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

/** `HH:MM` of `iso` as the UI shows a time of today (browser time zone, 24-hour clock). */
export function clock(page: Page, iso: string): Promise<string> {
  return page.evaluate(
    (time) => new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit" }).format(new Date(time)),
    iso,
  );
}

/** A provider call as the runner logs it: a claimed job attempt, or its end with the outcome. */
export interface RunnerEvent {
  at: number;
  event: "claimed" | "finished";
  job: string;
  attempt: number;
  kind?: string;
  outcome?: string;
}

/** The runner's job attempts so far (runner.log of a local stack). */
export function runnerEvents(stack: Stack): RunnerEvent[] {
  if (!stack.stateDir) throw new Error("the runner log of a stack started elsewhere is out of reach");
  const claimed = /^(\S+)\s+INFO runner::worker: job claimed job=(\S+) attempt=(\d+) kind="(\w+)"/;
  const finished =
    /^(\S+)\s+INFO runner::execute: job attempt finished job=(\S+) attempt=(\d+) outcome="(\w+)"/;
  return readFileSync(join(stack.stateDir, "logs", "runner.log"), "utf8")
    .split("\n")
    .flatMap((line): RunnerEvent[] => {
      const claim = claimed.exec(line);
      if (claim)
        return [
          { at: Date.parse(claim[1]), event: "claimed", job: claim[2], attempt: +claim[3], kind: claim[4] },
        ];
      const end = finished.exec(line);
      if (end)
        return [
          { at: Date.parse(end[1]), event: "finished", job: end[2], attempt: +end[3], outcome: end[4] },
        ];
      return [];
    });
}

/**
 * The attempts of the job that hit the usage limit: none may start before `resumesAt`, exactly one after it.
 * `kind` narrows to one job kind.
 */
export function expectRetriedOnlyAfter(stack: Stack, resumesAt: number, kind: string): void {
  const events = runnerEvents(stack);
  const limited = events.find((event) => event.event === "finished" && event.outcome === "usage_limited");
  expect(limited, "a job attempt ended on the usage limit").toBeDefined();
  const job = events.find((event) => event.event === "claimed" && event.job === limited?.job);
  expect(job?.kind).toBe(kind);
  const later = events.filter(
    (event) =>
      event.event === "claimed" && event.job === limited?.job && event.attempt > (limited?.attempt ?? 0),
  );
  expect(
    later.filter((event) => event.at < resumesAt).map((event) => new Date(event.at).toISOString()),
    `no attempt before ${new Date(resumesAt).toISOString()}`,
  ).toEqual([]);
  expect(later.length, "exactly one attempt after the reset").toBe(1);
}

/** Runs a read-only query on the server database of a local stack; rows as `|`-separated lines. */
export function sql(stack: Stack, query: string): string[] {
  if (!stack.stateDir) throw new Error("the database of a stack started elsewhere is out of reach");
  return execFileSync(
    "sqlite3",
    ["-readonly", join(stack.stateDir, "server", "smart-review.sqlite"), query],
    {
      encoding: "utf8",
    },
  )
    .split("\n")
    .filter(Boolean);
}
