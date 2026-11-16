import { createAnalysis } from "./create-analysis";
import { getAnalysis } from "./get-analysis";
import { getLines } from "./get-lines";
import { getMe } from "./get-me";
import { getPendingReview } from "./get-pending-review";
import { getReviewState } from "./get-review-state";
import { getTour } from "./get-tour";
import { listFeedback } from "./list-feedback";
import { listQuestions } from "./list-questions";
import { resolvePr } from "./resolve-pr";

/** Handlers of the endpoints that have an api-seed (fixtures/pr-482), one file per endpoint. */
export const handlers = [
  getMe,
  resolvePr,
  createAnalysis,
  getAnalysis,
  getTour,
  getLines,
  getReviewState,
  listQuestions,
  listFeedback,
  getPendingReview,
];
