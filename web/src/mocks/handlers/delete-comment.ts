import { HttpResponse } from "msw";
import { readStore, updateStore } from "../review-store";
import { apiError, forAnalysis, route } from "./common";

/** `DELETE …/pending-review/comments/{comment_id}`. */
export const deleteComment = route(
  "deleteComment",
  forAnalysis(({ params }) => {
    if (!readStore("pendingReview").comments.some((c) => c.id === params.comment_id)) {
      return apiError(404, "not_found", "This comment is no longer in your pending review.");
    }
    updateStore((store) => {
      store.pendingReview.comments = store.pendingReview.comments.filter((c) => c.id !== params.comment_id);
    });
    return new HttpResponse(null, { status: 204 });
  }),
);
