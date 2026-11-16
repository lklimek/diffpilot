import { forAnalysis, route, seedJson } from "./common";

/** `GET /api/v1/analyses/{id}/tour`. */
export const getTour = route("getTour", forAnalysis(seedJson("tour")));
