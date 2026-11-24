import { HttpResponse } from "msw";
import type { AssessmentConfig, CredentialStatus } from "../../api/generated/types";
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
  route("getAssessmentSettings", () => HttpResponse.json({ ...assessmentSettings, config })),
  route("setAssessmentSettings", async ({ request }) => {
    config = (await request.json()) as AssessmentConfig;
    return HttpResponse.json({ ...assessmentSettings, config });
  }),
  route("getAssessmentComparison", () => HttpResponse.json(assessmentComparison)),
  route("getCodex", () => HttpResponse.json(codex)),
  route("linkCodex", () => {
    polls = 0;
    codex = { ...unlinkedCodex, state: "linking", relink_reason: null };
    return HttpResponse.json({
      user_code: "FAKE-PLAN",
      verification_url: "http://127.0.0.1/fake-device",
      expires_at: new Date(Date.now() + 300_000).toISOString(),
    });
  }),
  route("getCodexLink", () => {
    if (polls++ === 0) return HttpResponse.json(codex);
    codex = { ...codex, state: "linked", label: "alice@example.invalid", plan: "fake", relink_reason: null };
    return HttpResponse.json(codex);
  }),
  route("deleteCodex", () => {
    codex = { ...unlinkedCodex };
    return new HttpResponse(null, { status: 204 });
  }),
];
