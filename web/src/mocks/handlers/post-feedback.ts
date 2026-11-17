import { HttpResponse } from "msw";
import type { Feedback, FeedbackRequest, FeedbackValue } from "../../api/generated/types";
import { updateStore } from "../review-store";
import { forAnalysis, route } from "./common";

const OPPOSITE: Partial<Record<FeedbackValue, FeedbackValue>> = {
  overrated: "underrated",
  underrated: "overrated",
};

/**
 * `POST /api/v1/analyses/{id}/feedback`: one entry per (target, value); posting a value again updates its note;
 * overrated and underrated exclude each other (ADR-011 CCR-F1).
 */
export const postFeedback = route(
  "postFeedback",
  forAnalysis(async ({ request }) => {
    const body = (await request.json()) as FeedbackRequest;
    const key = JSON.stringify(body.target);
    const entry: Feedback = {
      id: crypto.randomUUID(),
      target: body.target,
      value: body.value,
      note: body.note,
      created_at: new Date().toISOString(),
      memory: "pending",
      calibration_example: body.value === "overrated" || body.value === "underrated",
    };
    updateStore((store) => {
      store.feedback = [
        ...store.feedback.filter(
          (f) =>
            JSON.stringify(f.target) !== key || (f.value !== body.value && f.value !== OPPOSITE[body.value]),
        ),
        entry,
      ];
    });
    return HttpResponse.json(entry);
  }),
);
