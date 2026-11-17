import { HttpResponse } from "msw";
import { liveAnalysis } from "../live-analysis";
import { forAnalysis, route } from "./common";

/** `GET /api/v1/analyses/{id}`: the analysis now. */
export const getAnalysis = route(
  "getAnalysis",
  forAnalysis(() => HttpResponse.json(liveAnalysis())),
);
