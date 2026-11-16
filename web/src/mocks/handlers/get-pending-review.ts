import { forAnalysis, route, seedJson } from "./common";

/** `GET /api/v1/analyses/{id}/pending-review`. */
export const getPendingReview = route("getPendingReview", forAnalysis(seedJson("pending-review")));
