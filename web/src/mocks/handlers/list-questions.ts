import { forAnalysis, route, seedJson } from "./common";

/** `GET /api/v1/analyses/{id}/questions`. */
export const listQuestions = route("listQuestions", forAnalysis(seedJson("questions")));
