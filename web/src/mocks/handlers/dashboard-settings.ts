import { HttpResponse } from "msw";
import type {
  AccessToken,
  CreateAccessToken,
  CreatedAccessToken,
  DashboardSettings,
  DashboardSettingsView,
  SetJudgeEffortRequest,
  SetTimeZoneRequest,
} from "../../api/generated/types";
import { validateSettings } from "../../components/dashboard/settings-dashboard/validation";
import viewJson from "../../../../fixtures/dashboard/api/settings.json";
import tokensJson from "../../../../fixtures/dashboard/api/access-tokens.json";
import createdJson from "../../../../fixtures/dashboard/api/access-token-created.json";
import { SETTINGS } from "../scenarios";
import { apiError, route } from "./common";
import { noteDashboardTimeZone } from "./dashboard";

// Dashboard settings, ranking effort and access tokens (R6–R10, docs/v1.1-dashboard-plan.md §E.2) answered from
// fixtures/dashboard with the validation of §F.6. Screen scenarios: `?settings-scenario=<name>` (SCENARIOS).

const VIEW = viewJson as unknown as DashboardSettingsView;
const TOKENS = tokensJson as unknown as AccessToken[];
const CREATED = createdJson as unknown as CreatedAccessToken;

interface Scenario {
  view?: DashboardSettingsView;
  tokens?: AccessToken[];
  /** PUT answers 422 for a rule only the server checks. */
  rejectSave?: boolean;
  /** Status of both GETs: 503 (cannot be read) or 404 (routes not deployed). */
  loadStatus?: 404 | 503;
}

/** Ten live tokens: the per-user limit. */
const TEN_TOKENS: AccessToken[] = Array.from({ length: 10 }, (_, i) => ({
  ...TOKENS[0]!,
  id: `Lim0000${i}`,
  prefix: `srp_Lim0000${i}`,
  name: `agent ${i + 1}`,
  expires_at: "2099-01-01T00:00:00Z",
}));

/** Scenarios of the Settings screen's dashboard part. */
const SCENARIOS = {
  /** Never saved: the defaults, no time zone yet, nothing unreachable. */
  defaults: { view: { ...VIEW, settings: VIEW.defaults, memcan_projects: [], unreachable: [] } },
  /** MemCan not configured on the server: the TODO switch is off and says why. */
  "memcan-off": { view: { ...VIEW, memcan_configured: false } },
  /** Ten live tokens: creating another is refused. */
  "token-limit": { tokens: TEN_TOKENS },
  /** No tokens yet. */
  "no-tokens": { tokens: [] },
  /** The server refuses a rule the client cannot check. */
  "save-rejected": { rejectSave: true },
  /** Neither the settings nor the tokens can be read. */
  "load-fails": { loadStatus: 503 },
  /** The server has no dashboard routes yet (dark launch). */
  "not-deployed": { loadStatus: 404 },
} satisfies Record<string, Scenario>;

/** Name of a dashboard settings scenario. */
export type DashboardSettingsScenario = keyof typeof SCENARIOS;

let selected: Scenario = {};
let view: DashboardSettingsView = structuredClone(VIEW);
let tokens: AccessToken[] = structuredClone(TOKENS);

/** Selects a scenario (null = the fixtures) and restores its stored settings and tokens. */
export function setDashboardSettingsScenario(name: DashboardSettingsScenario | null): void {
  if (name !== null && !(name in SCENARIOS)) throw new Error(`unknown settings scenario "${String(name)}"`);
  selected = name === null ? {} : SCENARIOS[name];
  view = structuredClone(selected.view ?? VIEW);
  tokens = structuredClone(selected.tokens ?? TOKENS);
}

/** Restores the fixture settings and tokens between mock tests. */
export function resetDashboardSettings(): void {
  setDashboardSettingsScenario(null);
}

// In the browser the page's `?settings-scenario=` picks the scenario once, when the mocks load.
const requested = new URLSearchParams(globalThis.location?.search ?? "").get("settings-scenario");
if (requested !== null) setDashboardSettingsScenario(requested as DashboardSettingsScenario);

const loadFailure = () =>
  selected.loadStatus === 404
    ? new HttpResponse(null, { status: 404 })
    : apiError(503, "unavailable", "The dashboard settings cannot be read right now. Reload the page later.");

/** Effective MemCan projects (G17): repository short names, then the extras, deduplicated. */
const memcanProjects = (settings: DashboardSettings) => [
  ...new Set([...settings.repositories.map((r) => r.repo.name), ...settings.memcan_extra_projects]),
];

/** `GET /api/v1/settings/dashboard`. */
export const getDashboardSettings = route("getDashboardSettings", () =>
  selected.loadStatus ? loadFailure() : HttpResponse.json(view),
);

/** `PUT /api/v1/settings/dashboard`: the rules of §F.6; 422 `invalid_dashboard_settings` names the first field. */
export const setDashboardSettings = route("setDashboardSettings", async ({ request }) => {
  const settings = (await request.json()) as DashboardSettings;
  const [problem] = Object.values(validateSettings(settings, view.own_login));
  if (problem) return apiError(422, "invalid_dashboard_settings", problem);
  if (selected.rejectSave) {
    return apiError(422, "invalid_dashboard_settings", 'Time zone: "Etc/Unknown" is not a known time zone.');
  }
  view = { ...view, settings, memcan_projects: memcanProjects(settings) };
  return HttpResponse.json(view);
});

/** `PUT /api/v1/settings/dashboard/time-zone`: 204. */
export const setDashboardTimeZone = route("setDashboardTimeZone", async ({ request }) => {
  const { time_zone } = (await request.json()) as SetTimeZoneRequest;
  view = { ...view, settings: { ...view.settings, time_zone } };
  noteDashboardTimeZone(time_zone);
  return new HttpResponse(null, { status: 204 });
});

/** `PUT /api/v1/settings/llm/judge-effort`. */
export const setJudgeEffort = route("setJudgeEffort", async ({ request }) => {
  const { reasoning_effort } = (await request.json()) as SetJudgeEffortRequest;
  return HttpResponse.json({
    ...SETTINGS.llm_selection,
    judge_effort: reasoning_effort,
    changed_by: "alice",
    changed_at: new Date().toISOString(),
  });
});

const live = (token: AccessToken) => new Date(token.expires_at) > new Date();

/** `GET /api/v1/settings/access-tokens`. */
export const listAccessTokens = route("listAccessTokens", () =>
  selected.loadStatus ? loadFailure() : HttpResponse.json(tokens),
);

/**
 * `POST /api/v1/settings/access-tokens`: 201 with the secret once; 400 for a name, scope or expiry outside the
 * rules, 409 `conflict` for a name in use, 409 `token_limit_reached` at 10 live tokens.
 */
export const createAccessToken = route("createAccessToken", async ({ request }) => {
  const body = (await request.json()) as CreateAccessToken;
  const length = [...body.name].length;
  if (
    length < 1 ||
    length > 60 ||
    body.scopes.join() !== "read" ||
    ![30, 90, 365].includes(body.expires_in_days)
  ) {
    return apiError(
      400,
      "invalid_request",
      "Use a name of 1 to 60 characters, scope read and 30, 90 or 365 days.",
    );
  }
  if (tokens.some((t) => t.name === body.name)) {
    return apiError(409, "conflict", `You already have a token called "${body.name}".`);
  }
  if (tokens.filter(live).length >= 10) {
    return apiError(409, "token_limit_reached", "You have 10 tokens. Revoke one to create another.");
  }
  const id = Math.random().toString(36).slice(2, 10).padEnd(8, "0");
  const now = new Date();
  const token: AccessToken = {
    id,
    name: body.name,
    prefix: `srp_${id}`,
    scopes: body.scopes,
    created_at: now.toISOString(),
    expires_at: new Date(now.getTime() + body.expires_in_days * 86_400_000).toISOString(),
    last_used_at: null,
  };
  tokens = [token, ...tokens];
  const created: CreatedAccessToken = {
    token: CREATED.token.replace(CREATED.access_token.id, id),
    access_token: token,
  };
  return HttpResponse.json(created, { status: 201 });
});

/** `DELETE /api/v1/settings/access-tokens/{id}`: 204, or 404 for an unknown token. */
export const revokeAccessToken = route("revokeAccessToken", ({ params }) => {
  if (!tokens.some((t) => t.id === params.id)) {
    return apiError(404, "not_found", "This token does not exist.");
  }
  tokens = tokens.filter((t) => t.id !== params.id);
  return new HttpResponse(null, { status: 204 });
});
