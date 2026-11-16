import { forAnalysis, route, seedJson } from "./common";

/** `GET /api/v1/analyses/{id}/review-state`. */
export const getReviewState = route("getReviewState", forAnalysis(seedJson("review-state")));
