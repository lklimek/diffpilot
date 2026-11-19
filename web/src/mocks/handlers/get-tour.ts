import { lazyTour } from "../lazy-flows";
import { HttpResponse } from "msw";
import type { Tour } from "../../api/generated/types";
import { seedNow } from "../fixtures";
import { liveAnalysis } from "../live-analysis";
import { scenario } from "../scenarios";
import { apiError, forAnalysis, route } from "./common";

/** `GET /api/v1/analyses/{id}/tour`: `analysis_not_finished` until the plan and PR context are accepted. */
export const getTour = route(
  "getTour",
  forAnalysis(() => {
    if (!liveAnalysis().review_ready && liveAnalysis().state.state !== "finished") {
      return apiError(409, "analysis_not_finished", "This analysis has not finished yet.");
    }
    const seed = seedNow("tour") as Tour;
    return HttpResponse.json(scenario().lazy ? lazyTour(seed) : (scenario().tour?.(seed) ?? seed));
  }),
);
