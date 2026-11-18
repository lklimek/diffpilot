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
  const resume = page.getByRole("link", { name: /^(Resume|Open) review$/ });
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
    await page.getByRole("link", { name: /^(Resume|Open) review$/ }).click();
  }
  await expect(page).toHaveURL(/\/analyses\/[0-9a-f-]+$/);
  await navTo(page, /^PR context$/);
  await expect(heading(page)).toHaveText("PR context: acme/node-client #482");
  return new URL(page.url()).pathname.split("/").at(-1) ?? "";
}

/** Clicks a navigation entry. */
export async function navTo(page: Page, name: string | RegExp): Promise<void> {
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

/** The analysis waiting screen's progress section: the one place a percentage may show (brief §1, §4.2). */
const PROGRESS_SECTION = 'section[aria-label="Analysis progress"]';

/**
 * Brief §1 / plan §12.4 on the rendered page: no banned phrase and no percentage in the page text (hidden tooltip
 * text included; the progress section of the analysis waiting screen may show its percentage), and no `%` at all
 * in the top bar.
 */
export async function expectTrustWording(page: Page): Promise<void> {
  // Text nodes joined by spaces (hidden text too; block boundaries keep word boundaries) and the rendered text
  // (inline markup inside a word); the second pair leaves the progress section out.
  const [text, withoutProgress] = await page.locator("body").evaluate((body, progress) => {
    const collect = (root: HTMLElement, skip: boolean) => {
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, (node) =>
        node.parentElement?.closest("script, style") || (skip && node.parentElement?.closest(progress))
          ? NodeFilter.FILTER_REJECT
          : NodeFilter.FILTER_ACCEPT,
      );
      const nodes: string[] = [];
      while (walker.nextNode()) nodes.push(walker.currentNode.textContent ?? "");
      return nodes.join(" ");
    };
    const rest = body.cloneNode(true) as HTMLElement;
    rest.querySelectorAll(progress).forEach((section) => section.remove());
    const rendered = (body as HTMLElement).innerText;
    return [`${collect(body, false)}\n${rendered}`, `${collect(body, true)}\n${rest.textContent ?? ""}`];
  }, PROGRESS_SECTION);
  expect(
    [...wordingViolations(text, [WORDS]), ...wordingViolations(withoutProgress, [PERCENT])],
    "banned wording on the page",
  ).toEqual([]);
  const banner = page.getByRole("banner");
  if ((await banner.count()) > 0) {
    const top = await banner
      .first()
      .evaluate((bar) => `${bar.textContent ?? ""} ${(bar as HTMLElement).innerText}`);
    expect(wordingViolations(top, [WORDS, /%/u]), "banned wording in the top bar").toEqual([]);
  }
}

/** A control that must carry a non-empty tooltip (brief §4 rule 3, plan §12.4). */
export interface RequiredTip {
  /** The screen it belongs to: a selector present only there (`body`: every screen). */
  screen: string;
  /** What it is, for the failure message. */
  what: string;
  /** The tooltip trigger itself (the element with `aria-describedby`). */
  selector: string;
  /** Source of a regular expression the accessible name (aria-label, else text) must match. */
  name?: string;
  /** How many must be rendered on that screen; 0: none required, but each one rendered needs its tip. */
  min: number;
}

const REVIEW = 'nav[aria-label="Flows and steps"]';
const PROGRESS = 'section[aria-label="Analysis progress"]';
const HOME = 'form:has(input[type="url"])';

/** The controls of web/src/components that carry a tooltip, per screen. */
export const REQUIRED_TIPS: RequiredTip[] = [
  // Top bar (TopBar.tsx): every item but the brand and the repository link.
  { screen: "body", what: "top bar: PR number", selector: 'header a[href*="/pull/"]', min: 0 },
  {
    screen: "body",
    what: "top bar: label (head SHA, CI, flows reviewed)",
    selector: 'header [tabindex="0"]',
    min: 0,
  },
  { screen: "body", what: "top bar: button (pending review, help)", selector: "header button", min: 0 },
  { screen: "body", what: "top bar: user", selector: 'header a[href="/settings"]', min: 0 },
  { screen: REVIEW, what: "top bar: PR number", selector: 'header a[href*="/pull/"]', min: 1 },
  { screen: REVIEW, what: "top bar: head SHA", selector: 'header code[tabindex="0"]', min: 1 },
  {
    screen: REVIEW,
    what: "top bar: flows reviewed",
    selector: 'header [tabindex="0"]',
    name: "^flows reviewed",
    min: 1,
  },
  {
    screen: REVIEW,
    what: "top bar: pending review",
    selector: "header button",
    name: "^Pending review",
    min: 1,
  },
  { screen: REVIEW, what: "top bar: help", selector: "header button", name: "^Help", min: 1 },
  { screen: REVIEW, what: "top bar: user", selector: 'header a[href="/settings"]', min: 1 },
  // Navigation (NavList.tsx): quiet rows, the pending-review row, risk and group labels.
  {
    screen: REVIEW,
    what: "navigation: quiet row",
    selector: `${REVIEW} ul[aria-label="Quiet rows"] button, ${REVIEW} ul[aria-label="Not analysed"] button`,
    min: 0,
  },
  {
    screen: REVIEW,
    what: "navigation: pending review",
    selector: `${REVIEW} button`,
    name: "^Pending review on GitHub",
    min: 0,
  },
  { screen: REVIEW, what: "risk or other LLM label", selector: "[data-llm-label]", min: 1 },
  { screen: "body", what: "provenance badge", selector: "[data-badge]", min: 0 },
  // Block (BlockNav.tsx) and inspector (FeedbackButtons.tsx, AskForm.tsx).
  { screen: REVIEW, what: "Previous", selector: "main button", name: "^← Previous", min: 1 },
  { screen: REVIEW, what: "Next", selector: "main button", name: "^Next", min: 1 },
  {
    screen: "body",
    what: "feedback value",
    selector: '[role="group"][aria-label^="Feedback on"] button',
    min: 0,
  },
  { screen: "body", what: "Ask", selector: 'form button[type="submit"]', name: "^(Ask|Sending…)$", min: 0 },
  { screen: "body", what: "Ask again", selector: "button", name: "^(Ask again|Asking…)$", min: 0 },
  { screen: "body", what: "feedback: Save note", selector: "button", name: "^Save note$", min: 0 },
  { screen: "body", what: "inspector: Back to step", selector: "button", name: "^← Back to step$", min: 0 },
  // Comment composer (CommentComposer.tsx): Cancel, Add, Retry, Discard; Edit, Delete, Save, Cancel, Delete
  // from GitHub and Keep of the user's pending comments on the line.
  { screen: "body", what: "comment composer button", selector: "[data-comment-composer] button", min: 0 },
  // Pending-review block (PendingReviewBlock.tsx): Edit, Delete, and Save and Cancel while editing.
  {
    screen: REVIEW,
    what: "pending review: Edit, Delete",
    selector: 'main button[aria-label^="Edit comment on "], main button[aria-label^="Delete comment on "]',
    min: 0,
  },
  {
    screen: REVIEW,
    what: "pending review: Save, Cancel",
    selector: 'main li:has(> textarea[aria-label^="Edit comment on "]) button',
    min: 0,
  },
  { screen: REVIEW, what: "resume note: Dismiss", selector: "main button", name: "^Dismiss$", min: 0 },
  // Progress (AnalysisProgress.tsx) and Home (PrUrlForm.tsx).
  {
    screen: PROGRESS,
    what: "progress action (Cancel, Retry, Resume review)",
    selector: `${PROGRESS} button, ${PROGRESS} a[href]`,
    min: 1,
  },
  { screen: HOME, what: "Analyze", selector: `${HOME} button[type="submit"]`, name: "^Analyze$", min: 1 },
];

/**
 * Checks the tooltips of the rendered screen; returns the texts shown and the problems found:
 * - each control of `required` on the current screen is rendered (`min`) and names a non-empty tooltip;
 * - every `aria-describedby` id names an element, and a tooltip it names is not empty;
 * - every rendered tooltip trigger (or the focusable element inside it) takes keyboard focus, aria-disabled
 *   controls included; the tip shows on focus and hides when focus leaves. Only the content of a closed
 *   `<details>` (not rendered) is skipped. No key is pressed (Escape is the app's global "close" shortcut).
 */
export async function sweepTooltips(
  page: Page,
  required: RequiredTip[] = REQUIRED_TIPS,
): Promise<{ texts: string[]; problems: string[] }> {
  return page.evaluate(async (required) => {
    const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
    const settle = async () => {
      await frame();
      await frame();
    };
    const previous = document.activeElement as HTMLElement | null;
    const texts: string[] = [];
    const problems: string[] = [];
    const rendered = (element: Element) =>
      element.checkVisibility({ contentVisibilityAuto: true, visibilityProperty: true });
    const nameOf = (element: Element) =>
      (element.getAttribute("aria-label") ?? element.textContent ?? "").trim().replace(/\s+/g, " ");
    const described = (trigger: Element) =>
      (trigger.getAttribute("aria-describedby") ?? "")
        .split(/\s+/)
        .filter(Boolean)
        .map((id) => ({ id, element: document.getElementById(id) }));
    const tipOf = (trigger: Element) =>
      described(trigger)
        .map(({ element }) => element)
        .find((element) => element?.getAttribute("role") === "tooltip");

    for (const want of required) {
      if (!document.querySelector(want.screen)) continue;
      const name = want.name === undefined ? null : new RegExp(want.name, "u");
      const found = [...document.querySelectorAll(want.selector)].filter(
        (element) => rendered(element) && (!name || name.test(nameOf(element))),
      );
      if (found.length < want.min)
        problems.push(`required control missing: ${want.what} (${found.length} of ${want.min} rendered)`);
      for (const element of found) {
        const tip = tipOf(element);
        if (!tip) problems.push(`required tooltip missing: ${want.what} "${nameOf(element).slice(0, 60)}"`);
      }
    }

    const focusable = "button, a[href], input, select, textarea, [tabindex]:not([tabindex='-1'])";
    for (const trigger of document.querySelectorAll<HTMLElement>("[aria-describedby]")) {
      const name = nameOf(trigger).slice(0, 60);
      for (const { id, element } of described(trigger))
        if (!element) problems.push(`aria-describedby names no element: "${id}" on "${name}"`);
      const tip = tipOf(trigger);
      // Not a tooltip (help text), or not rendered: hidden, display none, or inside a closed <details>.
      if (!tip || !rendered(trigger)) continue;
      if ((tip.textContent ?? "").trim() === "") problems.push(`tooltip is empty: "${name}"`);
      // A trigger that is not focusable itself opens through a focusable element inside it (focus bubbles).
      const target = trigger.matches(focusable) ? trigger : trigger.querySelector<HTMLElement>(focusable);
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
  }, required);
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
export async function checkScreen(page: Page, required: RequiredTip[] = REQUIRED_TIPS): Promise<void> {
  await expectTrustWording(page);
  const tooltips = await sweepTooltips(page, required);
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
