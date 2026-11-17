import { HttpResponse } from "msw";
import { setLiveState, stopRun } from "../live-analysis";
import { forAnalysis, route } from "./common";

/** `POST /api/v1/analyses/{id}/cancel`: stops the run; the analysis is cancelled. */
export const cancelAnalysis = route(
  "cancelAnalysis",
  forAnalysis(() => {
    stopRun();
    return HttpResponse.json(setLiveState({ state: "cancelled" }));
  }),
);
