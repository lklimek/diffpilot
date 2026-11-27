import { setupServer } from "msw/node";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import type { AccessToken, DashboardSettingsView } from "../../api/generated/types";
import { handlers } from "./index";
import { resetDashboardSettings, setDashboardSettingsScenario } from "./dashboard-settings";

const BASE = location.origin;
const HEADERS = { "X-CSRF-Token": "fixture-csrf-token-482", "Content-Type": "application/json" };
const server = setupServer(...handlers);
beforeAll(() => server.listen({ onUnhandledFrame: "error" }));
afterEach(resetDashboardSettings);
afterAll(() => server.close());

async function call<T>(path: string, method = "GET", body?: unknown): Promise<{ status: number; body: T }> {
  const response = await fetch(`${BASE}${path}`, {
    method,
    headers: HEADERS,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  return { status: response.status, body: (text ? JSON.parse(text) : null) as T };
}

const SETTINGS = "/api/v1/settings/dashboard";
const TOKENS = "/api/v1/settings/access-tokens";

describe("msw dashboard settings (plan §F.6) and access tokens (ADR-019)", () => {
  it("saves only a requested dashboard settings section from an earlier snapshot", async () => {
    const { settings } = (await call<DashboardSettingsView>(SETTINGS)).body;
    const sync = await call<DashboardSettingsView>(`${SETTINGS}?section=github-sync`, "PUT", {
      ...settings,
      schedule_time: "07:15",
    });
    expect(sync.status).toBe(200);
    const memcan = await call<DashboardSettingsView>(`${SETTINGS}?section=memcan`, "PUT", {
      ...settings,
      include_todos: false,
    });
    expect(memcan.body.settings).toMatchObject({ schedule_time: "07:15", include_todos: false });
  });

  it("PUT checks every rule and names the first failing field; a save recomputes the MemCan projects", async () => {
    const { settings } = (await call<DashboardSettingsView>(SETTINGS)).body;
    expect(await call(SETTINGS, "PUT", { ...settings, linked_logins: ["alice"] })).toMatchObject({
      status: 422,
      body: { code: "invalid_dashboard_settings", message: expect.stringMatching(/^Linked logins/) },
    });
    const saved = await call<DashboardSettingsView>(SETTINGS, "PUT", {
      ...settings,
      repositories: [settings.repositories[0]],
      memcan_extra_projects: ["platform", "notes"],
    });
    expect(saved.body.memcan_projects).toEqual(["platform", "notes"]);
    expect((await call<DashboardSettingsView>(SETTINGS)).body.settings.repositories).toHaveLength(1);
  });

  it("scenarios: defaults, a server-only refusal, unreadable and not deployed", async () => {
    setDashboardSettingsScenario("defaults");
    const view = (await call<DashboardSettingsView>(SETTINGS)).body;
    expect(view.settings).toEqual(view.defaults);
    setDashboardSettingsScenario("save-rejected");
    expect((await call(SETTINGS, "PUT", view.settings)).status).toBe(422);
    setDashboardSettingsScenario("load-fails");
    expect((await call(SETTINGS)).status).toBe(503);
    expect((await call(TOKENS)).status).toBe(503);
    expect(() => setDashboardSettingsScenario("nope" as "defaults")).toThrow(/unknown settings scenario/);
  });

  it("token create refuses a bad request, a name in use and an eleventh live token", async () => {
    const request = { name: "agent", scopes: ["read"], expires_in_days: 90 };
    expect((await call(TOKENS, "POST", { ...request, scopes: ["write"] })).status).toBe(400);
    expect((await call(TOKENS, "POST", { ...request, expires_in_days: 7 })).status).toBe(400);
    expect((await call(TOKENS, "POST", { ...request, name: "" })).status).toBe(400);
    expect(await call(TOKENS, "POST", { ...request, name: "laptop scripts" })).toMatchObject({
      status: 409,
      body: { code: "conflict" },
    });
    setDashboardSettingsScenario("token-limit");
    expect((await call<AccessToken[]>(TOKENS)).body).toHaveLength(10);
    expect(await call(TOKENS, "POST", request)).toMatchObject({
      status: 409,
      body: { code: "token_limit_reached" },
    });
  });
});
