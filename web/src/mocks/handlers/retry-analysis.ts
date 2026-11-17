import { HttpResponse } from "msw";
import { liveAnalysis, setLiveState, startRun } from "../live-analysis";
import { apiError, forAnalysis, route } from "./common";

/** `POST /api/v1/analyses/{id}/retry`: a failed analysis plans again and runs to finished. */
export const retryAnalysis = route(
  "retryAnalysis",
  forAnalysis(() => {
    if (liveAnalysis().state.state !== "failed") {
      return apiError(409, "conflict", "Only a failed analysis can be retried.");
    }
    const summary = setLiveState({ state: "planning" });
    startRun();
    return HttpResponse.json(summary);
  }),
);
