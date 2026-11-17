import { HttpResponse } from "msw";
import type { UpdateCommentRequest } from "../../api/generated/types";
import { readStore, updateStore } from "../review-store";
import { apiError, forAnalysis, route } from "./common";

/** `PATCH …/pending-review/comments/{comment_id}`: replaces the comment text. */
export const updateComment = route(
  "updateComment",
  forAnalysis(async ({ request, params }) => {
    const { body } = (await request.json()) as UpdateCommentRequest;
    const comment = readStore("pendingReview").comments.find((c) => c.id === params.comment_id);
    if (!comment) return apiError(404, "not_found", "This comment is no longer in your pending review.");
    const updated = { ...comment, body };
    updateStore((store) => {
      store.pendingReview.comments = store.pendingReview.comments.map((c) =>
        c.id === updated.id ? updated : c,
      );
    });
    return HttpResponse.json(updated);
  }),
);
