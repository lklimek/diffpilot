import { setupServer } from "msw/node";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { routes, type RouteName } from "../../api/generated/routes";
import type {
  AccessToken,
  CreatedAccessToken,
  DashboardSettingsView,
  DashboardSummary,
  ItemPage,
  LlmSelection,
  RefreshStatus,
  ResolvePrResponse,
} from "../../api/generated/types";
import rankingJson from "../../../../fixtures/dashboard/api/refresh-ranking.json";
import { handlers } from "./index";
import { setFixtureState } from "../fixtures";
import { setScenario } from "../scenarios";
import { DASHBOARD_SCENARIOS, SCRIPT_STEP_MS, resetDashboard, setDashboardScenario } from "./dashboard";
import { resetDashboardSettings } from "./dashboard-settings";

const BASE = location.origin;
const CSRF = { "X-CSRF-Token": "fixture-csrf-token-482", "Content-Type": "application/json" };
const server = setupServer(...handlers);
beforeAll(() => server.listen({ onUnhandledFrame: "error" }));
afterEach(() => {
  vi.useRealTimers();
  setDashboardScenario("normal");
  setScenario(null);
  setFixtureState("default");
  resetDashboard();
  resetDashboardSettings();
});
afterAll(() => server.close());

const DASHBOARD_ROUTES: RouteName[] = [
  "getDashboard",
  "listDashboardItems",
  "startDashboardRefresh",
  "getDashboardRefresh",
  "cancelDashboardRefresh",
  "rankDashboardRefresh",
  "dashboardRefreshEvents",
  "getDashboardSettings",
  "setDashboardSettings",
  "setDashboardTimeZone",
  "setJudgeEffort",
  "listAccessTokens",
  "createAccessToken",
  "revokeAccessToken",
];

async function call<T>(path: string, init?: RequestInit): Promise<{ status: number; body: T }> {
  const response = await fetch(`${BASE}${path}`, init);
  const text = await response.text();
  return { status: response.status, body: (text ? JSON.parse(text) : null) as T };
}

describe("msw handlers of the dashboard routes (fixtures/dashboard)", () => {
  it("cover every dashboard and access-token route", () => {
    const covered = handlers.map((h) => h.info.header);
    for (const name of DASHBOARD_ROUTES) {
      const path = routes[name].path.replace(/\{(\w+)\}/g, ":$1");
      expect(covered).toContain(`${routes[name].method} ${path}`);
    }
  });

  it("serve the summary and page the items with a cursor of the current revision", async () => {
    const summary = (await call<DashboardSummary>("/api/v1/dashboard")).body;
    const first = (await call<ItemPage>("/api/v1/dashboard/items?limit=5")).body;
    expect(first.revision).toBe(summary.revision);
    expect(first.items).toHaveLength(5);
    const rest = (await call<ItemPage>(`/api/v1/dashboard/items?limit=100&cursor=${first.next_cursor}`)).body;
    expect(first.items.length + rest.items.length).toBe(summary.snapshot!.item_counts.total);
    expect(rest.next_cursor).toBeNull();
    const stale = await call<{ code: string }>("/api/v1/dashboard/items?cursor=old:5");
    expect(stale).toMatchObject({ status: 409, body: { code: "cursor_expired" } });
    const prs = (await call<ItemPage>("/api/v1/dashboard/items?kind=pr")).body;
    expect(new Set(prs.items.map((item) => item.kind))).toEqual(new Set(["pr", "todo"]));
    const both = (await call<ItemPage>("/api/v1/dashboard/items")).body;
    expect(new Set(both.items.map((item) => item.kind))).toEqual(new Set(["pr", "issue", "todo"]));
    const noTodos = (await call<ItemPage>("/api/v1/dashboard/items?todos=false")).body;
    expect(new Set(noTodos.items.map((item) => item.kind))).toEqual(new Set(["pr", "issue"]));
  });

  it("start one refresh at a time, read and cancel it", async () => {
    const started = await call<RefreshStatus>("/api/v1/dashboard/refreshes", {
      method: "POST",
      headers: CSRF,
    });
    expect(started).toMatchObject({ status: 201, body: { state: "scanning" } });
    const again = await call<{ code: string }>("/api/v1/dashboard/refreshes", {
      method: "POST",
      headers: CSRF,
    });
    expect(again).toMatchObject({ status: 409, body: { code: "refresh_running" } });
    const id = started.body.id;
    expect((await call<RefreshStatus>(`/api/v1/dashboard/refreshes/${id}`)).body.id).toBe(id);
    const cancelled = await call<RefreshStatus>(`/api/v1/dashboard/refreshes/${id}/cancel`, {
      method: "POST",
      headers: CSRF,
    });
    expect(cancelled.body.state).toBe("cancelled");
    expect((await call<DashboardSummary>("/api/v1/dashboard")).body.latest_refresh?.state).toBe("cancelled");
    const ended = await call<{ code: string }>(`/api/v1/dashboard/refreshes/${id}/cancel`, {
      method: "POST",
      headers: CSRF,
    });
    expect(ended).toMatchObject({ status: 409, body: { code: "conflict" } });
  });

  it("cancel a ranking: the refresh finishes with ranking cancelled; Retry ranking runs it again once", async () => {
    const ranking = rankingJson as unknown as RefreshStatus;
    const post = (path: string) =>
      call<RefreshStatus & { code?: string }>(path, { method: "POST", headers: CSRF });
    const base = `/api/v1/dashboard/refreshes/${ranking.id}`;
    expect(await post(`${base}/rank`)).toMatchObject({ status: 409, body: { code: "conflict" } });
    const cancelled = await post(`${base}/cancel`);
    expect(cancelled.body).toMatchObject({ state: "finished", ranking: { state: "cancelled" } });
    const retried = await post(`${base}/rank`);
    expect(retried).toMatchObject({ status: 202, body: { state: "ranking", ranking: { state: "running" } } });
    expect(await post(`${base}/rank`)).toMatchObject({ status: 409, body: { code: "conflict" } });
  });

  it("a new refresh supersedes a live ranking; only a scan blocks it", async () => {
    const ranking = rankingJson as unknown as RefreshStatus;
    const post = (path: string) =>
      call<RefreshStatus & { code?: string }>(path, { method: "POST", headers: CSRF });
    const base = `/api/v1/dashboard/refreshes/${ranking.id}`;
    await post(`${base}/cancel`);
    expect((await post(`${base}/rank`)).body.state).toBe("ranking");
    expect(await post("/api/v1/dashboard/refreshes")).toMatchObject({
      status: 201,
      body: { state: "scanning" },
    });
    expect((await call<RefreshStatus>(base)).body).toMatchObject({
      state: "finished",
      ranking: { state: "cancelled" },
    });
    expect(await post(`${base}/rank`)).toMatchObject({ status: 409, body: { code: "refresh_running" } });
    expect(await post("/api/v1/dashboard/refreshes")).toMatchObject({
      status: 409,
      body: { code: "refresh_running" },
    });
  });

  it("save settings, reject an invalid look-back and set the ranking effort", async () => {
    const view = (await call<DashboardSettingsView>("/api/v1/settings/dashboard")).body;
    const put = (settings: unknown) =>
      call<DashboardSettingsView & { code?: string }>("/api/v1/settings/dashboard", {
        method: "PUT",
        headers: CSRF,
        body: JSON.stringify(settings),
      });
    expect((await put({ ...view.settings, lookback_days: 14 })).body.settings.lookback_days).toBe(14);
    expect(await put({ ...view.settings, lookback_days: 0 })).toMatchObject({
      status: 422,
      body: { code: "invalid_dashboard_settings" },
    });
    const effort = await call<LlmSelection>("/api/v1/settings/llm/judge-effort", {
      method: "PUT",
      headers: CSRF,
      body: JSON.stringify({ reasoning_effort: "medium" }),
    });
    expect(effort.body.judge_effort).toBe("medium");
  });

  it("show a created token's secret once and revoke it", async () => {
    const created = await call<CreatedAccessToken>("/api/v1/settings/access-tokens", {
      method: "POST",
      headers: CSRF,
      body: JSON.stringify({ name: "agent", scopes: ["read"], expires_in_days: 90 }),
    });
    expect(created.status).toBe(201);
    expect(created.body.token.startsWith(`${created.body.access_token.prefix}_`)).toBe(true);
    const listed = (await call<AccessToken[]>("/api/v1/settings/access-tokens")).body;
    expect(listed.map((t) => t.id)).toContain(created.body.access_token.id);
    expect(JSON.stringify(listed)).not.toContain(created.body.token);
    const path = `/api/v1/settings/access-tokens/${created.body.access_token.id}`;
    expect((await call(path, { method: "DELETE", headers: CSRF })).status).toBe(204);
    expect((await call(path, { method: "DELETE", headers: CSRF })).status).toBe(404);
  });
});

describe("dashboard scenarios (?dashboard=<name>, the mockup states)", () => {
  const summary = async () => (await call<DashboardSummary>("/api/v1/dashboard")).body;
  const items = async () => (await call<ItemPage>("/api/v1/dashboard/items?limit=500")).body;
  const post = (path: string) =>
    call<RefreshStatus & { code?: string }>(path, { method: "POST", headers: CSRF });

  it("serve items of the summary's revision in every scenario", async () => {
    for (const name of DASHBOARD_SCENARIOS) {
      setDashboardScenario(name);
      const s = await summary();
      const page = await items();
      expect(page.revision, name).toBe(s.revision);
      expect(page.items.length, name).toBe(s.snapshot?.item_counts.total ?? 0);
    }
  });

  it("show each mockup state through the latest refresh and its ranking", async () => {
    const states: Record<string, unknown> = {};
    for (const name of [
      "normal",
      "refreshing",
      "provisional",
      "waiting",
      "error",
      "empty",
      "first-run",
    ] as const) {
      setDashboardScenario(name);
      const s = await summary();
      states[name] = [s.latest_refresh?.state ?? null, s.latest_refresh?.ranking?.state ?? null];
    }
    expect(states).toEqual({
      normal: ["finished", "finished"],
      refreshing: ["scanning", null],
      provisional: ["ranking", "running"],
      waiting: ["ranking", "waiting_for_analysis"],
      error: ["failed", null],
      empty: ["finished", "finished"],
      "first-run": [null, null],
    });
    setDashboardScenario("provisional");
    expect((await items()).items.filter((i) => i.score.state !== "final").map((i) => i.id)).toEqual([
      "WUI-320",
      "NC-482",
      "CON-1534",
      "PLAT-1210",
    ]);
  });

  it("answer 304 to a matching If-None-Match", async () => {
    const first = await fetch(`${BASE}/api/v1/dashboard/items`);
    const etag = first.headers.get("ETag");
    expect(etag).toMatch(/^W\/"/);
    const again = await fetch(`${BASE}/api/v1/dashboard/items`, { headers: { "If-None-Match": etag! } });
    expect(again.status).toBe(304);
    const other = await fetch(`${BASE}/api/v1/dashboard/items?kind=issue`, {
      headers: { "If-None-Match": etag! },
    });
    expect(other.status).toBe(200);
  });

  it("refuse a refresh before GitHub's rate-limit reset (error scenario)", async () => {
    setDashboardScenario("error");
    expect((await summary()).latest_refresh?.failure?.retry_at).not.toBeNull();
    expect(await post("/api/v1/dashboard/refreshes")).toMatchObject({
      status: 429,
      body: { code: "github_rate_limited" },
    });
  });

  it("run a started refresh through scanning, a provisional ranking and a final ranking", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    const started = await post("/api/v1/dashboard/refreshes");
    expect(started.body).toMatchObject({ state: "scanning", percent: 10 });
    const before = await summary();
    expect(before.snapshot?.refresh_id).not.toBe(started.body.id);

    vi.advanceTimersByTime(5 * SCRIPT_STEP_MS);
    const ranking = await summary();
    expect(ranking.latest_refresh).toMatchObject({ id: started.body.id, state: "ranking" });
    expect(ranking.snapshot).toMatchObject({ refresh_id: started.body.id, ranked_at: null });
    expect((await items()).items.some((i) => i.score.state !== "final")).toBe(true);

    vi.advanceTimersByTime(2 * SCRIPT_STEP_MS);
    const done = await summary();
    expect(done.latest_refresh).toMatchObject({
      state: "finished",
      ranking: { state: "finished", judged: 13 },
    });
    expect(done.revision).not.toBe(ranking.revision);
    const final = await items();
    expect(final.revision).toBe(done.revision);
    expect(final.items.every((i) => i.score.state === "final")).toBe(true);
  });

  it("Rank again runs a failed ranking to its end", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    setDashboardScenario("ranking-failed");
    const id = (await summary()).latest_refresh!.id;
    expect(await post(`/api/v1/dashboard/refreshes/${id}/rank`)).toMatchObject({
      status: 202,
      body: { state: "ranking", ranking: { state: "running", message: null } },
    });
    vi.advanceTimersByTime(2 * SCRIPT_STEP_MS);
    expect((await summary()).latest_refresh).toMatchObject({
      id,
      state: "finished",
      ranking: { state: "finished" },
    });
  });

  it("store the browser's time zone through R6b in the summary's schedule", async () => {
    setDashboardScenario("no-time-zone");
    expect((await summary()).schedule).toMatchObject({ time_zone: null, next_run_at: null });
    const put = await call("/api/v1/settings/dashboard/time-zone", {
      method: "PUT",
      headers: CSRF,
      body: JSON.stringify({ time_zone: "Europe/Warsaw" }),
    });
    expect(put.status).toBe(204);
    expect((await summary()).schedule.time_zone).toBe("Europe/Warsaw");
    expect((await summary()).schedule.next_run_at).not.toBeNull();
  });

  it("review scenarios: NC-482's review link and the review start page agree (new, found, moved)", async () => {
    const nc482 = async () => (await items()).items.find((i) => i.id === "NC-482")!.review;
    const resolve = async () =>
      (
        await call<ResolvePrResponse>("/api/v1/pr/resolve", {
          method: "POST",
          headers: CSRF,
          body: JSON.stringify({ url: "https://github.com/acme/node-client/pull/482" }),
        })
      ).body;
    setDashboardScenario("review-new");
    expect(await nc482()).toBeNull();
    expect((await resolve()).analysis).toBeNull();
    setDashboardScenario("review-found");
    expect(await nc482()).toMatchObject({ head_moved: false });
    expect(await resolve()).toMatchObject({ head_moved: false, analysis: { state: { state: "finished" } } });
    setDashboardScenario("review-moved");
    expect(await nc482()).toMatchObject({ head_moved: true });
    expect((await resolve()).head_moved).toBe(true);
  });
});
