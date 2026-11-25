// TODO(v1.4 phase B): replace with generated contract type (plan C9)
import type { AssessmentProgress, AssessmentState } from "../generated/types";

export type CommentAssessmentState = AssessmentState | { state: "queued" };
export type CommentAssessmentProgress = AssessmentProgress & {
  in_pass?: number;
  waiting?: number;
  since?: string | null;
};
