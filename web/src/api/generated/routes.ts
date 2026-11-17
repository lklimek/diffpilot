// This file is generated from crates/contracts by the test generated_artifacts_are_current (UPDATE_GENERATED=1 rewrites it). Do not edit.

/** Browser-facing routes: method, path with `{param}` placeholders, authentication. */
export const routes = {
  healthz: { method: "GET", path: "/healthz", auth: "public" },
  authLogin: { method: "GET", path: "/auth/github/login", auth: "public" },
  authCallback: { method: "GET", path: "/auth/github/callback", auth: "public" },
  authLogout: { method: "POST", path: "/auth/logout", auth: "session_csrf" },
  getMe: { method: "GET", path: "/api/v1/me", auth: "public" },
  resolvePr: { method: "POST", path: "/api/v1/pr/resolve", auth: "session_csrf" },
  createAnalysis: { method: "POST", path: "/api/v1/analyses", auth: "session_csrf" },
  getAnalysis: { method: "GET", path: "/api/v1/analyses/{id}", auth: "session" },
  analysisEvents: { method: "GET", path: "/api/v1/analyses/{id}/events", auth: "session" },
  cancelAnalysis: { method: "POST", path: "/api/v1/analyses/{id}/cancel", auth: "session_csrf" },
  pauseAnalysis: { method: "POST", path: "/api/v1/analyses/{id}/pause", auth: "session_csrf" },
  resumeAnalysis: { method: "POST", path: "/api/v1/analyses/{id}/resume", auth: "session_csrf" },
  retryAnalysis: { method: "POST", path: "/api/v1/analyses/{id}/retry", auth: "session_csrf" },
  getTour: { method: "GET", path: "/api/v1/analyses/{id}/tour", auth: "session" },
  getLines: { method: "GET", path: "/api/v1/analyses/{id}/lines", auth: "session" },
  getDiff: { method: "GET", path: "/api/v1/analyses/{id}/diff", auth: "session" },
  getReviewState: { method: "GET", path: "/api/v1/analyses/{id}/review-state", auth: "session" },
  putReviewState: { method: "PUT", path: "/api/v1/analyses/{id}/review-state", auth: "session_csrf" },
  listQuestions: { method: "GET", path: "/api/v1/analyses/{id}/questions", auth: "session" },
  askQuestion: { method: "POST", path: "/api/v1/analyses/{id}/steps/{step_id}/questions", auth: "session_csrf" },
  listFeedback: { method: "GET", path: "/api/v1/analyses/{id}/feedback", auth: "session" },
  postFeedback: { method: "POST", path: "/api/v1/analyses/{id}/feedback", auth: "session_csrf" },
  deleteFeedback: { method: "DELETE", path: "/api/v1/analyses/{id}/feedback/{feedback_id}", auth: "session_csrf" },
  getPendingReview: { method: "GET", path: "/api/v1/analyses/{id}/pending-review", auth: "session" },
  createComment: { method: "POST", path: "/api/v1/analyses/{id}/pending-review/comments", auth: "session_csrf" },
  updateComment: { method: "PATCH", path: "/api/v1/analyses/{id}/pending-review/comments/{comment_id}", auth: "session_csrf" },
  deleteComment: { method: "DELETE", path: "/api/v1/analyses/{id}/pending-review/comments/{comment_id}", auth: "session_csrf" },
  getSettings: { method: "GET", path: "/api/v1/settings", auth: "session" },
  setLanguage: { method: "PUT", path: "/api/v1/settings/language", auth: "session_csrf" },
  setLlm: { method: "PUT", path: "/api/v1/settings/llm", auth: "session_csrf" },
  getRepoSettings: { method: "GET", path: "/api/v1/repos/{owner}/{name}/settings", auth: "session" },
} as const;

/** Name of a browser-facing route. */
export type RouteName = keyof typeof routes;
