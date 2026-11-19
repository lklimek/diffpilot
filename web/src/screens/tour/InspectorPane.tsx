import type { UseQueryResult } from "@tanstack/react-query";
import { useState, type RefObject } from "react";
import type {
  AnalysisId,
  Annotation,
  AskQuestion,
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
import { AskForm } from "../../components/ask/AskForm";
import { AskThread, type AskScope } from "../../components/ask/AskThread";
import {
  CommentComposer,
  type CommentDrafts,
  type UnresolvedPosts,
} from "../../components/comment/CommentComposer";
import { commentPlacement, isChangedLine } from "../../components/comment/placement";
import type { QuestionPlace } from "../../components/questions/QuestionsBlock";
import { FeedbackButtons } from "../../components/feedback/FeedbackButtons";
import { Inspector, type InspectorProps } from "../../components/inspector/Inspector";
import type { SectionControl } from "../../components/inspector/sectionState";
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
  onFlashed?: () => void;
  onBackToStep: () => void;
  onSelectStep: (step: StepId) => void;
  /** The step of the tour a question was asked on (its link in the Ask conversation). */
  placeOf: (step: StepId) => QuestionPlace | null;
  onEvidence: (evidence: Evidence) => void;
  codeShown: boolean;
  pending: UseQueryResult<PendingReview>;
  comments: ReturnType<typeof useCommentActions>;
  commentLine: CommentLineState | null;
  onCancelComment: () => void;
  unresolved: UnresolvedPosts;
  onUnresolvedChange: (update: (posts: UnresolvedPosts) => UnresolvedPosts) => void;
  drafts: CommentDrafts;
  /** Where the PR can be analysed again (after a head-moved refusal of a comment). */
  reanalyseHref?: string;
  askRef: RefObject<HTMLTextAreaElement | null>;
  /** Open / collapsed Inspector sections, Ask included (the `a` shortcut reveals Ask). */
  sections: SectionControl;
  commentRef: RefObject<HTMLTextAreaElement | null>;
  /** Dragged height of the Ask area (remembered with the layout); null = default. */
  askHeight: number | null;
  onAskHeight: (height: number | null) => void;
  /** "Ask about this line" from a code line of the step. */
  onAskLine?: (target: CommentTarget) => void;
  /** Text for the Ask field from "Ask about this line"; `onAskInserted` once it is in. */
  askInsert: { text: string } | null;
  onAskInserted: () => void;
  /** "Use as review comment" on a flag or an Ask answer of the step; absent while commenting is unavailable. */
  onFlagAsComment?: (flag: Annotation) => void;
  onAnswerAsComment?: (question: AskQuestion) => void;
  /** Draft text for the comment composer ("Use as review comment"); `onCommentPrefilled` once it is in. */
  commentPrefill: { text: string } | null;
  onCommentPrefilled: () => void;
}

/** GitHub link to a line at the analysed commit of its side. */
function permalink(tour: Tour, target: CommentTarget): string {
  const { repo, head_sha, base_sha } = tour.analysis.pr;
  const sha = target.side === "base" ? base_sha : head_sha;
  const path = target.path.split("/").map(encodeURIComponent).join("/");
  return `https://github.com/${repo.owner}/${repo.name}/blob/${sha}/${path}#L${target.line}`;
}

/** The inspector with its data: classic diff and the dock (Ask with its pinned answers, feedback, comment). */
export function InspectorPane(props: InspectorPaneProps) {
  const { analysisId, tour, block, step, annotationId, pending, comments, commentLine } = props;
  const questions = useQuestions(analysisId);
  const ask = useAskQuestion(analysisId);
  const feedback = useFeedback(analysisId);
  const postFeedback = usePostFeedback(analysisId);
  const deleteFeedback = useDeleteFeedback(analysisId);
  const annotation = step?.llm.annotations.find((a) => a.id === annotationId);
  const [askScope, setAskScope] = useState<AskScope>("conversation");

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
        onAskLine: props.onAskLine,
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
    ) : questions.data.length === 0 ? null : (
      <AskThread
        questions={questions.data}
        stepId={step.id}
        scope={askScope}
        onScope={setAskScope}
        placeOf={props.placeOf}
        onEvidence={props.onEvidence}
        currentFile={step.facts.file}
        onAskAgain={onAsk}
        onUseAsComment={props.onAnswerAsComment}
      />
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
      drafts={props.drafts}
      reanalyseHref={props.reanalyseHref}
      prefill={props.commentPrefill}
      onPrefilled={props.onCommentPrefilled}
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
      sections={props.sections}
      onFlagAsComment={props.onFlagAsComment}
      dockHeight={props.askHeight}
      onDockHeight={step ? props.onAskHeight : undefined}
      tour={tour}
      block={block}
      annotationId={annotationId}
      flash={props.flash}
      onFlashed={props.onFlashed}
      diff={diff}
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
            insert={props.askInsert}
            onInserted={props.onAskInserted}
            answers={answers}
            sections={props.sections}
            notice={
              props.streamLost
                ? "The connection that delivers answers was lost. Reconnecting; answers already given are read again."
                : null
            }
            count={
              step && questions.data ? questions.data.filter((q) => q.step_id === step.id).length : undefined
            }
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
