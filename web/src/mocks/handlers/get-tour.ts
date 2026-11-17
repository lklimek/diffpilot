import { HttpResponse } from "msw";
import type { Tour } from "../../api/generated/types";
import { seedNow } from "../fixtures";
import { liveAnalysis } from "../live-analysis";
import { scenario } from "../scenarios";
import { apiError, forAnalysis, route } from "./common";

/** `GET /api/v1/analyses/{id}/tour`: `analysis_not_finished` until the analysis has finished. */
export const getTour = route(
  "getTour",
  forAnalysis(() => {
    if (liveAnalysis().state.state !== "finished") {
      return apiError(409, "analysis_not_finished", "This analysis has not finished yet.");
    }
    const seed = seedNow("tour") as Tour;
    return HttpResponse.json(scenario().tour?.(seed) ?? seed);
  }),
);
