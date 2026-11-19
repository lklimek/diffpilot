import { HttpResponse } from "msw";
import type {
  CreateCommentRequest,
  CreateCommentResponse,
  PendingComment,
  Tour,
} from "../../api/generated/types";
import { seedNow } from "../fixtures";
import { mockLines } from "../lines";
import { commentResponses, readStore, updateStore } from "../review-store";
import { scenario } from "../scenarios";
import { apiError, forAnalysis, route } from "./common";

/** Whether an LLM annotation of the tour covers the line. */
function annotated(path: string, side: string, line: number): boolean {
  return (seedNow("tour") as Tour).flows.some((flow) =>
    flow.steps.some(
      (step) =>
        step.facts.file === path &&
        step.facts.side === side &&
        step.llm.annotations.some((a) => a.range.start_line <= line && line <= a.range.end_line),
    ),
  );
}

/**
 * `POST …/pending-review/comments` (ADR-006): a line thread inside one hunk, else a file comment linking to the line;
 * a file outside the diff is `position_rejected`; a changed line without annotation records `missed_by_llm`.
 * One idempotency key answers its first response.
 */
export const createComment = route(
  "createComment",
  forAnalysis(async ({ request }) => {
    const body = (await request.json()) as CreateCommentRequest;
    const first = commentResponses.get(body.idempotency_key);
    if (first) return HttpResponse.json(first, { status: 201 });
    if (scenario().commentPublished) {
      return apiError(
        409,
        "comment_published",
        "The pull request changed while the comment was being saved, and your review was published on GitHub in the meantime. The comment had been published on other code and was removed again. Analyse the pull request again to comment.",
      );
    }
    if (!readStore("pendingReview").commenting_available) {
      return apiError(
        422,
        "app_not_installed",
        "The diffpilot GitHub App is not installed on this repository.",
      );
    }
    const { path, side, start_line, line } = body.target;
    const file = mockLines(path, side, 1, Number.MAX_SAFE_INTEGER);
    if (!file || file.comments.scope === "not_in_diff") {
      return apiError(422, "position_rejected", "GitHub does not accept review comments on this file.");
    }
    const from = start_line ?? line;
    const inHunk =
      side === "base" ||
      file.comments.line_thread_ranges.some((r) => r.start_line <= from && line <= r.end_line);
    const id = crypto.randomUUID();
    const comment: PendingComment = {
      id: `PRRC_${id}`,
      thread_id: `PRRT_${id}`,
      path,
      kind: inHunk ? "line" : "file",
      side: inHunk ? side : null,
      start_line,
      line,
      body: body.body,
    };
    const missed = file.changed.includes(line) && !annotated(path, side, line);
    updateStore((store) => {
      store.pendingReview.comments.push(comment);
      if (missed) {
        store.feedback.push({
          id: crypto.randomUUID(),
          target: { target: "line", path, side, line },
          value: "missed_by_llm",
          note: null,
          created_at: new Date().toISOString(),
          memory: "pending",
          calibration_example: false,
        });
      }
    });
    const response: CreateCommentResponse = { comment, missed_by_llm_recorded: missed };
    commentResponses.set(body.idempotency_key, response);
    return HttpResponse.json(response, { status: 201 });
  }),
);
