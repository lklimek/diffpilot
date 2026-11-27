import { HttpResponse } from "msw";
import { setLiveState, startRun } from "../live-analysis";
import type { AnalysisSummary, CreateAnalysisRequest } from "../../api/generated/types";
import { seedNow } from "../fixtures";
import { scenario } from "../scenarios";
import { route } from "./common";

/** `POST /api/v1/analyses`: restarts the fixture analysis, scripted to finish unless the scenario pins a state. */
export const createAnalysis = route("createAnalysis", async ({ request }) => {
  const pinned = scenario().analysis;
  const body = (await request.json()) as CreateAnalysisRequest;
  const seed = seedNow("analysis") as AnalysisSummary;
  const summary = setLiveState(pinned ?? { state: "planning" }, { ...seed.pr, head_sha: body.head_sha });
  if (!pinned) startRun();
  return HttpResponse.json(summary, { status: 201 });
});
