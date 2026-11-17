import { HttpResponse } from "msw";
import { readStore } from "../review-store";
import { forAnalysis, route } from "./common";

/** `GET /api/v1/analyses/{id}/review-state`. */
export const getReviewState = route(
  "getReviewState",
  forAnalysis(() => HttpResponse.json(readStore("reviewState"))),
);
