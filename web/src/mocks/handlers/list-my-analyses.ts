import { HttpResponse } from "msw";
import type { AnalysisSummary, MyAnalyses, MyAnalysis } from "../../api/generated/types";
import { liveAnalysis } from "../live-analysis";
import { scenario } from "../scenarios";
import { apiError, route } from "./common";

/** Older analyses of the mock user besides the fixture PR (synthetic; opening them answers not_found). */
const OLDER: MyAnalysis[] = [
  {
    id: "00000000-0000-4000-8000-000000000096",
    pr: { repo: { owner: "acme", name: "wallet-ui" }, number: 96 },
    title: "refactor(send-form): split validation",
    status: "running",
    created_at: "2026-11-16T09:12:00Z",
  },
  {
    id: "00000000-0000-4000-8000-000000000455",
    pr: { repo: { owner: "acme", name: "node-client" }, number: 455 },
    title: "chore: bump tokio to 1.41",
    status: "failed",
    created_at: "2026-11-12T15:40:00Z",
  },
];

/** The fixture analysis as a list row. */
const row = (analysis: AnalysisSummary): MyAnalysis => ({
  id: analysis.id,
  pr: { repo: analysis.pr.repo, number: analysis.pr.number },
  title: analysis.pr.title,
  status: analysis.state.state,
  created_at: analysis.created_at,
});

export const myAnalyses = (): MyAnalyses =>
  scenario().myAnalyses?.() ?? { analyses: [row(liveAnalysis()), ...OLDER] };

/** `GET /api/v1/analyses`: the fixture analysis and two older ones, newest first (scenario overrides). */
export const listMyAnalyses = route("listMyAnalyses", ({ request }) => {
  const limit = Number(new URL(request.url).searchParams.get("limit") ?? 20);
  if (!Number.isInteger(limit) || limit < 1 || limit > 100) {
    return apiError(400, "invalid_request", "The limit must be between 1 and 100.");
  }
  const body = myAnalyses();
  return HttpResponse.json({ analyses: body.analyses.slice(0, limit) });
});
