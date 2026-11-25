import { HttpResponse } from "msw";
import type { AssessmentSettingsUpdate, CredentialStatus } from "../../api/generated/types";
import { assessmentComparison, assessmentSettings, unlinkedCodex } from "../assessment";
import { route } from "./common";
let config = structuredClone(assessmentSettings.config);
let codex: CredentialStatus = { ...unlinkedCodex };
let polls = 0;
/** Fresh mock session for component and route tests. */
export function resetAssessmentSettings() {
  config = structuredClone(assessmentSettings.config);
  codex = { ...unlinkedCodex };
  polls = 0;
}
/** Token-free local fixtures; no upstream authentication calls. */
export const assessmentHandlers = [
  route("getAssessmentSettings", () =>
    HttpResponse.json({ ...assessmentSettings, config, revision: JSON.stringify(config) }),
  ),
  route("setAssessmentSettings", async ({ request }) => {
    const update = (await request.json()) as AssessmentSettingsUpdate;
    if (update.revision !== JSON.stringify(config))
      return HttpResponse.json(
        { code: "conflict", message: "Settings changed in another tab. Reload and try again." },
        { status: 409 },
      );
    config = update.config;
    return HttpResponse.json({ ...assessmentSettings, config, revision: JSON.stringify(config) });
  }),
  route("getAssessmentComparison", () => HttpResponse.json(assessmentComparison)),
  route("getCodex", () => HttpResponse.json(codex)),
  route("linkCodex", () => {
    polls = 0;
    codex = { ...unlinkedCodex, state: "linking", relink_reason: null };
    const challenge = {
      user_code: "FAKE-PLAN",
      verification_url: "http://127.0.0.1/fake-device",
      expires_at: new Date(Date.now() + 300_000).toISOString(),
    };
    codex.challenge = challenge;
    return HttpResponse.json(challenge);
  }),
  route("getCodexLink", () => {
    if (codex.state !== "linking" || polls++ === 0) return HttpResponse.json(codex);
    codex = {
      ...codex,
      challenge: null,
      state: "linked",
      telemetry: { ...codex.telemetry, primary: { used_percent: 12.5, resets_at: "2026-11-19T00:00:00Z" } },
      label: "alice@example.invalid",
      plan: "fake",
      relink_reason: null,
    };
    return HttpResponse.json(codex);
  }),
  route("deleteCodex", () => {
    codex = { ...unlinkedCodex };
    return new HttpResponse(null, { status: 204 });
  }),
];
