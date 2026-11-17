import { HttpResponse } from "msw";
import { readStore } from "../review-store";
import { forAnalysis, route } from "./common";

/** `GET /api/v1/analyses/{id}/feedback`. */
export const listFeedback = route(
  "listFeedback",
  forAnalysis(() => HttpResponse.json(readStore("feedback"))),
);
