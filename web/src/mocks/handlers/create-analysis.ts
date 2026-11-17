import { HttpResponse } from "msw";
import { setLiveState, startRun } from "../live-analysis";
import { scenario } from "../scenarios";
import { route } from "./common";

/** `POST /api/v1/analyses`: restarts the fixture analysis, scripted to finish unless the scenario pins a state. */
export const createAnalysis = route("createAnalysis", () => {
  const pinned = scenario().analysis;
  const summary = setLiveState(pinned ?? { state: "planning" });
  if (!pinned) startRun();
  return HttpResponse.json(summary, { status: 201 });
});
