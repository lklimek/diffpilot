import { HttpResponse } from "msw";
import type { Tour } from "../../api/generated/types";
import { seedNow } from "../fixtures";
import { mockFiles } from "../files";
import { lazyTour } from "../lazy-flows";
import { liveAnalysis } from "../live-analysis";
import { scenario } from "../scenarios";
import { apiError, forAnalysis, route } from "./common";

/** `GET /api/v1/analyses/{id}/files` (R12): `analysis_not_finished` until the plan and PR context are accepted. */
export const getFiles = route(
  "getFiles",
  forAnalysis(() => {
    if (!liveAnalysis().review_ready && liveAnalysis().state.state !== "finished") {
      return apiError(409, "analysis_not_finished", "This analysis has not finished yet.");
    }
    const seed = seedNow("tour") as Tour;
    return HttpResponse.json(mockFiles(scenario().lazy ? lazyTour(seed) : seed));
  }),
);
