import { HttpResponse } from "msw";
import { liveAnalysis, setLiveState, startRun } from "../live-analysis";
import { apiError, forAnalysis, route } from "./common";
/** Resumes from the saved count. */
export const resumeAnalysis = route(
  "resumeAnalysis",
  forAnalysis(() => {
    const state = liveAnalysis().state;
    if (state.state !== "paused" || state.reason !== "user")
      return apiError(409, "conflict", "Only a user-paused analysis can be resumed.");
    const summary = setLiveState(
      state.total === null
        ? { state: "planning" }
        : { state: "running", done: state.done, total: state.total },
    );
    startRun(state.done + 1, state.total ?? 5);
    return HttpResponse.json(summary);
  }),
);
