// TODO(v1.4 phase B): replace with generated contract type (plan C10)
import { api } from "../client";
import type {
  CommentItem,
  CommentsOverview,
  CommentsSummary,
  CommentTally,
  PrPage,
  PrRef,
} from "../generated/types";
import type { CommentAssessmentProgress, CommentAssessmentState } from "./commentAssessment";
import type { CommentDuplicateFields } from "./commentDuplicates";
import type { CommentsWatchFields } from "./commentsWatch";
import type { CommentTitleFields } from "./commentTitle";
import type { ImportedGithubDecision } from "./importedDecision";

type CommentFields = CommentTitleFields &
  CommentDuplicateFields & {
    status_comment?: boolean;
    imported_decision?: ImportedGithubDecision | null;
  };

export type CommentItemResponse = Omit<CommentItem, "assessment_state"> &
  CommentFields & {
    assessment_state: CommentAssessmentState;
  };
export type ReviewComment = CommentItem & CommentFields & { assessment_queued?: boolean };

export type ReviewCommentsSummary = Omit<CommentsSummary, "assessment"> & {
  assessment: CommentAssessmentProgress;
};
export type ReviewCommentsOverview = Omit<CommentsOverview, "assessment"> &
  CommentsWatchFields & {
    assessment: CommentAssessmentProgress;
  };
export type ReviewPrPage = Omit<PrPage, "comments"> & { comments: ReviewCommentsSummary };

/** Server tally used before the review queue is read. */
export type ReviewCommentTally = CommentTally;

export const fetchReviewPrPage = (pr: PrRef): Promise<ReviewPrPage> => api.getPrPage(pr);
export const fetchReviewComments = (pr: PrRef): Promise<ReviewCommentsOverview> => api.getComments(pr);

export function asReviewComment(item: CommentItemResponse): ReviewComment {
  return item.assessment_state.state === "queued"
    ? { ...item, assessment_queued: true, assessment_state: { state: "not_assessed", reason: "not_run" } }
    : { ...item, assessment_state: item.assessment_state };
}

export async function fetchReviewCommentItems(pr: PrRef): Promise<ReviewComment[]> {
  const items: CommentItemResponse[] = await api.listCommentItems(pr);
  return items.map(asReviewComment);
}
