import { HttpResponse } from "msw";
import { readStore } from "../review-store";
import { forAnalysis, route } from "./common";

/** `GET /api/v1/analyses/{id}/pending-review`. */
export const getPendingReview = route(
  "getPendingReview",
  forAnalysis(() => HttpResponse.json(readStore("pendingReview"))),
);
