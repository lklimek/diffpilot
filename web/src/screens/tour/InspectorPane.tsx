import type { UseQueryResult } from "@tanstack/react-query";
import type { RefObject } from "react";
import type {
  AnalysisId,
  AnnotationId,
  BlockRef,
  CommentTarget,
  Evidence,
  FeedbackTarget,
  LinesResponse,
  PendingReview,
  Step,
  StepId,
  Tour,
} from "../../api/generated/types";
import {
  useAskQuestion,
  useFeedback,
  useDeleteFeedback,
  usePostFeedback,
  useQuestions,
  type useCommentActions,
} from "../../api/queries";
import { AskAnswers } from "../../components/ask/AskAnswers";
import { AskForm } from "../../components/ask/AskForm";
import { CommentComposer, type UnresolvedPosts } from "../../components/comment/CommentComposer";
import { commentPlacement, isChangedLine } from "../../components/comment/placement";
import { FeedbackButtons } from "../../components/feedback/FeedbackButtons";
import { Inspector, type InspectorProps } from "../../components/inspector/Inspector";
import { StatusMessage } from "../../components/status/StatusMessage";
import { DiffCode, errorText } from "./code";
import { stepQuery } from "./sequence";

/** The line a comment is being written on, with the displayed lines it was chosen in. */
export interface CommentLineState {
  target: CommentTarget;
  lines: LinesResponse;
}

interface InspectorPaneProps {
  streamLost?: boolean;
  analysisId: AnalysisId;
  tour: Tour;
  block: BlockRef;
  /** The current step, when the block is one. */
  step: Step | undefined;
  annotationId: AnnotationId | null;
  /** Remark or thread to scroll to and highlight ("Show in Inspector"). */
  flash?: InspectorProps["flash"];
  onBackToStep: () => void;
  onSelectStep: (step: StepId) => void;
  onEvidence: (evidence: Evidence) => void;
  codeShown: boolean;
  pending: UseQueryResult<PendingReview>;
  comments: ReturnType<typeof useCommentActions>;
  commentLine: CommentLineState | null;
  onCancelComment: () => void;
  unresolved: UnresolvedPosts;
  onUnresolvedChange: (update: (posts: UnresolvedPosts) => UnresolvedPosts) => void;
  askRef: RefObject<HTMLTextAreaElement | null>;
  commentRef: RefObject<HTMLTextAreaElement | null>;
}

/** GitHub link to a line at the analysed commit of its side. */
function permalink(tour: Tour, target: CommentTarget): string {
  const { repo, head_sha, base_sha } = tour.analysis.pr;
  const sha = target.side === "base" ? base_sha : head_sha;
  const path = target.path.split("/").map(encodeURIComponent).join("/");
  return `https://github.com/${repo.owner}/${repo.name}/blob/${sha}/${path}#L${target.line}`;
}

/** The inspector with its data: classic diff, pinned answers, and the dock (Ask, feedback, comment). */
export function InspectorPane(props: InspectorPaneProps) {
  const { analysisId, tour, block, step, annotationId, pending, comments, commentLine } = props;
  const questions = useQuestions(analysisId);
  const ask = useAskQuestion(analysisId);
  const feedback = useFeedback(analysisId);
  const postFeedback = usePostFeedback(analysisId);
  const deleteFeedback = useDeleteFeedback(analysisId);
  const annotation = step?.llm.annotations.find((a) => a.id === annotationId);

  const onAsk = (stepId: StepId, text: string) => ask.mutateAsync({ step: stepId, text });
  // Feedback on a plain line only where the server takes it: a changed line of the diff on its side.
  const line =
    commentLine && isChangedLine(commentLine.lines, commentLine.target) ? commentLine.target : null;
  const target: FeedbackTarget | null = annotation
    ? { target: "annotation", annotation_id: annotation.id }
    : line
      ? { target: "line", path: line.path, side: line.side, line: line.line }
      : step
        ? { target: "step", step_id: step.id }
        : null;

  const diff = step && (
    <DiffCode
      analysisId={analysisId}
      comments={{
        comments: pending.data?.comments ?? [],
        onEditComment: comments.edit,
        onDeleteComment: comments.remove,
      }}
      query={
        annotation
          ? { ...stepQuery(step), from: annotation.range.start_line, to: annotation.range.end_line }
          : stepQuery(step)
      }
    />
  );
  const answers =
    step &&
    (questions.isError ? (
      <StatusMessage kind="error">Cannot read the questions: {errorText(questions.error)}</StatusMessage>
    ) : questions.isPending ? (
      <StatusMessage kind="loading">Loading the questions…</StatusMessage>
    ) : !props.streamLost && !questions.data.some((q) => q.step_id === step.id) ? null : (
      <>
        {props.streamLost && (
          <p role="status" className="mb-1 text-[12px] text-high">
            The connection that delivers answers was lost. Reconnecting; answers already given are read again.
          </p>
        )}
        <AskAnswers
          onEvidence={props.onEvidence}
          questions={questions.data.filter((q) => q.step_id === step.id)}
          onAskAgain={onAsk}
        />
      </>
    ));

  const comment = pending.isSuccess ? (
    <CommentComposer
      available={pending.data.commenting_available}
      line={
        commentLine && {
          target: commentLine.target,
          placement: commentPlacement(commentLine.lines, commentLine.target),
          permalink: permalink(tour, commentLine.target),
        }
      }
      comments={pending.data.comments}
      onCreate={comments.create}
      onEdit={comments.edit}
      onDelete={comments.remove}
      onCancel={props.onCancelComment}
      textareaRef={props.commentRef}
      unresolved={props.unresolved}
      onUnresolvedChange={props.onUnresolvedChange}
    />
  ) : pending.isError ? (
    <p className="text-[12.5px]">
      Commenting is not available right now: your pending review could not be read. {errorText(pending.error)}
    </p>
  ) : (
    <StatusMessage kind="loading">Loading your pending review…</StatusMessage>
  );

  return (
    <Inspector
      tour={tour}
      block={block}
      annotationId={annotationId}
      flash={props.flash}
      diff={diff}
      answers={answers}
      pendingReview={pending.data ?? null}
      onBackToStep={props.onBackToStep}
      onSelectStep={props.onSelectStep}
      onEvidence={props.onEvidence}
      dock={
        <>
          {/* While a comment is written its composer leads the dock, so its line reference is in view. */}
          {commentLine && comment}
          <AskForm
            step={step ? { id: step.id, label: `Step ${step.number} · ${step.llm.title}` } : null}
            onAsk={onAsk}
            inputRef={props.askRef}
          />
          <FeedbackButtons
            target={target}
            subject={
              annotation
                ? "this annotation"
                : line
                  ? `line ${line.line}`
                  : step
                    ? `step ${step.number}`
                    : "this block"
            }
            feedback={feedback.data ?? []}
            onSubmit={postFeedback.mutateAsync}
            onRemove={deleteFeedback.mutateAsync}
          />
          {feedback.isError && (
            <p role="alert" className="text-[12px] text-high">
              Stored feedback could not be read: {errorText(feedback.error)}
            </p>
          )}
          {!commentLine && props.codeShown && comment}
        </>
      }
    />
  );
}
