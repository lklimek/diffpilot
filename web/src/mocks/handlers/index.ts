import { requestFlow, setBackgroundAnalysis } from "./flow-work";
import { setAskEffort, setLlm } from "./set-llm";
import { pauseAnalysis } from "./pause-analysis";
import { resumeAnalysis } from "./resume-analysis";
import { analysisEvents } from "./analysis-events";
import { askAnchored } from "./ask-anchored";
import { askQuestion } from "./ask-question";
import { cancelAnalysis } from "./cancel-analysis";
import { createAnalysis } from "./create-analysis";
import { createComment } from "./create-comment";
import {
  cancelDashboardRefresh,
  dashboardRefreshEvents,
  getDashboard,
  hideDashboardMention,
  getDashboardRefresh,
  listDashboardItems,
  rankDashboardRefresh,
  snoozeDashboardItem,
  startDashboardRefresh,
  wakeDashboardItem,
  watchDashboardPr,
} from "./dashboard";
import {
  createAccessToken,
  getDashboardSettings,
  listAccessTokens,
  revokeAccessToken,
  setDashboardSettings,
  setDashboardTimeZone,
  setJudgeEffort,
} from "./dashboard-settings";
import { deleteComment } from "./delete-comment";
import { deleteFeedback } from "./delete-feedback";
import { getAnalysis } from "./get-analysis";
import { getDiff } from "./get-diff";
import { getDependencies } from "./get-dependencies";
import { getFiles } from "./get-files";
import { getLines } from "./get-lines";
import { authLogout, getMe } from "./get-me";
import { getPendingReview } from "./get-pending-review";
import { getRepoSettings } from "./get-repo-settings";
import { getReviewState } from "./get-review-state";
import { getSettings } from "./get-settings";
import { getTour } from "./get-tour";
import { listFeedback } from "./list-feedback";
import { listMyAnalyses } from "./list-my-analyses";
import { listQuestions } from "./list-questions";
import { postFeedback } from "./post-feedback";
import { putReviewState } from "./put-review-state";
import { resolvePr } from "./resolve-pr";
import { reviewLoopHandlers } from "./review-loop";
import { retryAnalysis } from "./retry-analysis";
import { setControlledOwners } from "./set-controlled-owners";
import { setFileViewed } from "./set-file-viewed";
import { setLanguage } from "./set-language";
import { updateComment } from "./update-comment";

/** Handlers of the mocked endpoints (fixtures/pr-482 seed and screen scenarios), one file per endpoint. */
export const handlers = [
  requestFlow,
  setBackgroundAnalysis,
  setLlm,
  pauseAnalysis,
  resumeAnalysis,
  getMe,
  authLogout,
  resolvePr,
  createAnalysis,
  listMyAnalyses,
  getAnalysis,
  getTour,
  getLines,
  getDiff,
  getReviewState,
  putReviewState,
  listQuestions,
  askQuestion,
  askAnchored,
  getFiles,
  setFileViewed,
  getDependencies,
  listFeedback,
  postFeedback,
  deleteFeedback,
  getPendingReview,
  createComment,
  updateComment,
  deleteComment,
  analysisEvents,
  cancelAnalysis,
  retryAnalysis,
  getSettings,
  setLanguage,
  setControlledOwners,
  getRepoSettings,
  getDashboard,
  hideDashboardMention,
  snoozeDashboardItem,
  wakeDashboardItem,
  listDashboardItems,
  startDashboardRefresh,
  getDashboardRefresh,
  cancelDashboardRefresh,
  rankDashboardRefresh,
  dashboardRefreshEvents,
  getDashboardSettings,
  setDashboardSettings,
  setDashboardTimeZone,
  setJudgeEffort,
  setAskEffort,
  listAccessTokens,
  createAccessToken,
  revokeAccessToken,
  watchDashboardPr,
  ...reviewLoopHandlers,
];
