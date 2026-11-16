import { forAnalysis, route, seedJson } from "./common";

/** `GET /api/v1/analyses/{id}`. */
export const getAnalysis = route("getAnalysis", forAnalysis(seedJson("analysis")));
