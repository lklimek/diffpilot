import { forAnalysis, route, seedJson } from "./common";

/** `GET /api/v1/analyses/{id}/feedback`. */
export const listFeedback = route("listFeedback", forAnalysis(seedJson("feedback")));
