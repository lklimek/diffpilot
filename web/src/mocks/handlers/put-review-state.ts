import { HttpResponse } from "msw";
import type { ReviewState } from "../../api/generated/types";
import { updateStore } from "../review-store";
import { forAnalysis, route } from "./common";

/** `PUT /api/v1/analyses/{id}/review-state`: replaces ticks and last viewed block. */
export const putReviewState = route(
  "putReviewState",
  forAnalysis(async ({ request }) => {
    const body = (await request.json()) as ReviewState;
    updateStore((store) => {
      store.reviewState = { ticked_steps: body.ticked_steps, last_viewed: body.last_viewed };
    });
    return new HttpResponse(null, { status: 204 });
  }),
);
