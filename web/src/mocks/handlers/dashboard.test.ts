import { setupServer } from "msw/node";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { routes, type RouteName } from "../../api/generated/routes";
import type {
  AccessToken,
  CreatedAccessToken,
  DashboardSettingsView,
  DashboardSummary,
  ItemPage,
  LlmSelection,
  RefreshStatus,
} from "../../api/generated/types";
import rankingJson from "../../../../fixtures/dashboard/api/refresh-ranking.json";
import { handlers } from "./index";
import { resetDashboard } from "./dashboard";
import { resetDashboardSettings } from "./dashboard-settings";

const BASE = location.origin;
const CSRF = { "X-CSRF-Token": "fixture-csrf-token-482", "Content-Type": "application/json" };
const server = setupServer(...handlers);
beforeAll(() => server.listen({ onUnhandledFrame: "error" }));
afterEach(() => {
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
