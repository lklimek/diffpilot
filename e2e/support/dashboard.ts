// Steps and checks of the dashboard journeys (My work, plan docs/v1.1-dashboard-plan.md): the synthetic world of
// fake GitHub (fixtures/dashboard/github) and fake MemCan, refreshes through the API, the ranked list as shown.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, type Page } from "@playwright/test";
import type { Stack } from "./stack";

const repo = (name: string, alias: string, watchIssues = false) => ({
  repo: { owner: "acme", name },
  alias,
  watch_issues: watchIssues,
  ownership_policy: null as null | { repo: { owner: string; name: string }; path: string },
  branch_patterns: [] as string[],
});

/** Dashboard settings of the world (as `crates/server/src/dashboard/refresh/tests.rs`). */
export function worldSettings() {
  const platform = repo("platform", "PLAT", true);
  platform.ownership_policy = {
    repo: { owner: "acme", name: "review-policies" },
    path: "policies/platform.json",
  };
  return {
    linked_logins: ["alice-helper"],
    repositories: [
      platform,
      repo("wallet-ui", "WUI"),
      repo("node-client", "NC"),
      repo("consensus", "CON"),
      repo("docs-site", "DOC"),
      repo("ops-runbooks", "OPS"),
      repo("secret", "SEC"),
    ],
    enumerate_owners: ["acme"],
    lookback_days: 7,
    ready_label: "ready-for-human",
    skip_authors: ["dependabot", "dependabot[bot]"],
    include_todos: true,
    memcan_extra_projects: [] as string[],
    schedule_enabled: true,
    schedule_time: "06:00",
    time_zone: "Europe/Warsaw" as string | null,
  };
}
export type DashboardSettings = ReturnType<typeof worldSettings>;

/**
 * Text the judge job carries (scanned facts of the private repository, a TODO description) and writes (the fake
 * judgement): it may reach the database and the browser, never a log.
 */
export const JUDGE_DATA: [string, string][] = [
  ["private pull request title (judge input)", "rotate the staging certificates"],
  ["TODO description (judge input)", "List the three breaking keys of config v3 and how to migrate them"],
  ["fake judgement (judge output)", "Fake judgement for tests."],
];

/** The session's CSRF token (`GET /api/v1/me`). */
export async function csrf(page: Page): Promise<string> {
  const response = await page.request.get("/api/v1/me");
  expect(response.ok()).toBe(true);
  const { csrf_token } = (await response.json()) as { csrf_token: string };
  return csrf_token;
}

/** Sends a session request with the CSRF token; returns status and JSON body (null when empty). */
export async function api(
  page: Page,
  method: "POST" | "PUT" | "DELETE",
  path: string,
  data?: unknown,
): Promise<{ status: number; body: Record<string, unknown> | null }> {
  const response = await page.request.fetch(path, {
    method,
    headers: { "x-csrf-token": await csrf(page) },
    data,
  });
  const text = await response.text();
  return { status: response.status(), body: text ? (JSON.parse(text) as Record<string, unknown>) : null };
}

/** Saves the dashboard settings through R6 (the settings screen has its own journey). */
export async function saveSettings(page: Page, settings: DashboardSettings): Promise<void> {
  const { status, body } = await api(page, "PUT", "/api/v1/settings/dashboard", settings);
  expect(status, JSON.stringify(body)).toBe(200);
}

type Todo = Record<string, unknown>;

/** Puts the fixture TODOs (fixtures/dashboard/scan/todos.json) into fake MemCan, each through `change`. */
export async function seedTodos(page: Page, stack: Stack, change = (todo: Todo) => todo): Promise<void> {
  if (!stack.memcan) throw new Error("this stack has no reachable fake MemCan");
  const projects = JSON.parse(
    readFileSync(new URL("../../fixtures/dashboard/scan/todos.json", import.meta.url), "utf8"),
  ) as Record<string, { todos: Todo[] }>;
  const response = await page.request.post(`${stack.memcan.url}/_fake/todos`, {
    headers: { authorization: `Bearer ${stack.memcan.token}` },
    data: Object.values(projects).flatMap((project) => project.todos.map(change)),
  });
  expect(response.status()).toBe(204);
}

/** Queues fake GitHub dashboard faults (`POST /_fake/dashboard/faults`, tools/fake-github/README.md). */
export async function githubFaults(page: Page, stack: Stack, faults: unknown[]): Promise<void> {
  if (!stack.github) throw new Error("this stack's fake GitHub is out of reach");
  const response = await page.request.post(`${stack.github}/_fake/dashboard/faults`, { data: { faults } });
  expect(response.status()).toBe(204);
}

/** A refresh as R2b reports it (the fields the journeys read). */
export interface Refresh {
  id: string;
  state: string;
  judged: number;
  reused: number;
  scanned_at: string | null;
  ranking: { state: string; judged: number; total: number } | null;
  failure: { code: string; message: string; retry_at: string | null } | null;
}

/** The user's newest refresh (R1 `latest_refresh`). */
export async function latestRefresh(page: Page): Promise<Refresh | null> {
  const response = await page.request.get("/api/v1/dashboard");
  expect(response.ok()).toBe(true);
  return ((await response.json()) as { latest_refresh: Refresh | null }).latest_refresh;
}

/** Polls the newest refresh until `done` holds; fails with the last one seen. */
export async function waitForRefresh(
  page: Page,
  done: (refresh: Refresh) => boolean,
  timeout = 120_000,
): Promise<Refresh> {
  const deadline = Date.now() + timeout;
  let last: Refresh | null = null;
  while (Date.now() < deadline) {
    last = await latestRefresh(page);
    if (last && done(last)) return last;
    await page.waitForTimeout(500);
  }
  throw new Error(`the refresh did not get there within ${timeout / 1000} s; last: ${JSON.stringify(last)}`);
}

/** Ranked: the refresh finished and so did its ranking. */
export const ranked = (refresh: Refresh) =>
  refresh.state === "finished" && refresh.ranking?.state === "finished";

/** The list section "Needs you". */
export const workList = (page: Page) => page.getByRole("region", { name: "Needs you" });

/** The display ids of the listed rows, in the order shown. */
export function rowIds(page: Page): Promise<string[]> {
  return workList(page)
    .locator("li[data-item]")
    .evaluateAll((rows) =>
      rows.map((row) =>
        (row.querySelector("[aria-label^='Details of ']")?.getAttribute("aria-label") ?? "").slice(
          "Details of ".length,
        ),
      ),
    );
}

/** The rows of the list group headed `name` (a tier, or "Not judged yet"), in order. */
export function groupIds(page: Page, name: string): Promise<string[]> {
  return workList(page)
    .locator("div", { has: page.getByRole("heading", { name: new RegExp(`^${name}\\b`) }) })
    .last()
    .locator("li[data-item]")
    .evaluateAll((rows) =>
      rows.map((row) =>
        (row.querySelector("[aria-label^='Details of ']")?.getAttribute("aria-label") ?? "").slice(
          "Details of ".length,
        ),
      ),
    );
}

/** The top bar's Refresh button. */
export const refreshButton = (page: Page) => page.getByRole("button", { name: "Refresh", exact: true });

/** Puts `login` on the server's allow-list (as deploy/dev/stack.sh does for `alice`). */
export function allowLogin(stack: Stack, login: string): void {
  if (!stack.stateDir) throw new Error("the database of a stack started elsewhere is out of reach");
  execFileSync("sqlite3", [
    "-cmd",
    ".timeout 5000",
    join(stack.stateDir, "server", "smart-review.sqlite"),
    `INSERT OR IGNORE INTO tenant_allowed_logins (tenant_id, login) VALUES ('default', '${login}');`,
  ]);
}

/** Makes fake GitHub sign in `login` from now on (`POST /_fake/user`). */
export async function nextGithubUser(page: Page, stack: Stack, login: string, id: number): Promise<void> {
  if (!stack.github) throw new Error("this stack's fake GitHub is out of reach");
  const response = await page.request.post(`${stack.github}/_fake/user`, { data: { login, id } });
  expect(response.status()).toBe(204);
}
