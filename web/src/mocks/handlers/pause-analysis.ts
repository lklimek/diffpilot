import { HttpResponse } from "msw";
import { liveAnalysis, setLiveState, stopRun } from "../live-analysis";
import { apiError, forAnalysis, route } from "./common";
/** A user pause stops the scripted mock at the current completed-flow count. */
export const pauseAnalysis = route(
  "pauseAnalysis",
  forAnalysis(() => {
    const state = liveAnalysis().state;
    if (!["planning", "running", "paused"].includes(state.state))
      return apiError(409, "conflict", "The analysis has already ended.");
    stopRun();
    return HttpResponse.json(
      setLiveState({
        state: "paused",
        reason: "user",
        paused_by: "alice",
        paused_at: new Date().toISOString(),
        resumes_at: null,
        done: "done" in state ? state.done : 0,
        total: "total" in state ? state.total : null,
      }),
    );
  }),
);
