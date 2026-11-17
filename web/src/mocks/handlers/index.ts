import { analysisEvents } from "./analysis-events";
import { askQuestion } from "./ask-question";
import { cancelAnalysis } from "./cancel-analysis";
import { createAnalysis } from "./create-analysis";
import { createComment } from "./create-comment";
import { deleteComment } from "./delete-comment";
import { getAnalysis } from "./get-analysis";
import { getDiff } from "./get-diff";
import { getLines } from "./get-lines";
import { getMe } from "./get-me";
import { getPendingReview } from "./get-pending-review";
import { getRepoSettings } from "./get-repo-settings";
import { getReviewState } from "./get-review-state";
import { getSettings } from "./get-settings";
import { getTour } from "./get-tour";
import { listFeedback } from "./list-feedback";
import { listQuestions } from "./list-questions";
import { postFeedback } from "./post-feedback";
import { putReviewState } from "./put-review-state";
import { resolvePr } from "./resolve-pr";
import { retryAnalysis } from "./retry-analysis";
import { setLanguage } from "./set-language";
import { updateComment } from "./update-comment";

/** Handlers of the mocked endpoints (fixtures/pr-482 seed and screen scenarios), one file per endpoint. */
export const handlers = [
  getMe,
  resolvePr,
  createAnalysis,
  getAnalysis,
  getTour,
  getLines,
  getDiff,
  getReviewState,
  putReviewState,
  listQuestions,
  askQuestion,
  listFeedback,
  postFeedback,
  getPendingReview,
  createComment,
  updateComment,
  deleteComment,
  analysisEvents,
  cancelAnalysis,
  retryAnalysis,
  getSettings,
  setLanguage,
  getRepoSettings,
];
