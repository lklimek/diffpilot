import { HttpResponse } from "msw";
import { readStore } from "../review-store";
import { forAnalysis, route } from "./common";

/** `GET /api/v1/analyses/{id}/questions`. */
export const listQuestions = route(
  "listQuestions",
  forAnalysis(() => HttpResponse.json(readStore("questions"))),
);
