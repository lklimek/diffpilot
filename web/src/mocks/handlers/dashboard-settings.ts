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
import viewJson from "../../../../fixtures/dashboard/api/settings.json";
import tokensJson from "../../../../fixtures/dashboard/api/access-tokens.json";
import createdJson from "../../../../fixtures/dashboard/api/access-token-created.json";
import { SETTINGS } from "../scenarios";
import { apiError, route } from "./common";
import { noteDashboardTimeZone } from "./dashboard";

// Dashboard settings, ranking effort and access tokens (R6–R10, docs/v1.1-dashboard-plan.md §E.2) answered from
// fixtures/dashboard; WP11-WEB-SET adds the screen scenarios and the full validation of §F.6.

const VIEW = viewJson as unknown as DashboardSettingsView;
const TOKENS = tokensJson as unknown as AccessToken[];
const CREATED = createdJson as unknown as CreatedAccessToken;

let view: DashboardSettingsView = structuredClone(VIEW);
let tokens: AccessToken[] = structuredClone(TOKENS);

/** Restores the fixture settings and tokens between mock tests. */
export function resetDashboardSettings(): void {
  view = structuredClone(VIEW);
  tokens = structuredClone(TOKENS);
}

/** `GET /api/v1/settings/dashboard`. */
export const getDashboardSettings = route("getDashboardSettings", () => HttpResponse.json(view));

/** `PUT /api/v1/settings/dashboard`: 422 `invalid_dashboard_settings` names the field (one rule shown here). */
export const setDashboardSettings = route("setDashboardSettings", async ({ request }) => {
  const settings = (await request.json()) as DashboardSettings;
  if (
    !Number.isInteger(settings.lookback_days) ||
    settings.lookback_days < 1 ||
    settings.lookback_days > 30
  ) {
    return apiError(422, "invalid_dashboard_settings", "Look-back days: use a whole number from 1 to 30.");
  }
  view = { ...view, settings };
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

/** `GET /api/v1/settings/access-tokens`. */
export const listAccessTokens = route("listAccessTokens", () => HttpResponse.json(tokens));

/** `POST /api/v1/settings/access-tokens`: 201 with the secret once; 409 `token_limit_reached` at 10 tokens. */
export const createAccessToken = route("createAccessToken", async ({ request }) => {
  const body = (await request.json()) as CreateAccessToken;
  if (tokens.length >= 10) {
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
