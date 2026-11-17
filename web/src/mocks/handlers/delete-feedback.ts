import { HttpResponse } from "msw";
import { readStore, updateStore } from "../review-store";
import { apiError, forAnalysis, route } from "./common";

/** `DELETE …/feedback/{feedback_id}`: removes one value; `missed_by_llm` is recorded by the server only. */
export const deleteFeedback = route(
  "deleteFeedback",
  forAnalysis(({ params }) => {
    const entry = readStore("feedback").find((f) => f.id === params.feedback_id);
    if (!entry || entry.value === "missed_by_llm") {
      return apiError(404, "not_found", "This feedback is no longer stored.");
    }
    updateStore((store) => {
      store.feedback = store.feedback.filter((f) => f.id !== params.feedback_id);
    });
    return new HttpResponse(null, { status: 204 });
  }),
);
