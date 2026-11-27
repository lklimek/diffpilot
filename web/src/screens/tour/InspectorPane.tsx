import { errorText } from "../../api/client";
import type { UseQueryResult } from "@tanstack/react-query";
import type { ReactNode, RefObject } from "react";
import type {
  AnalysisId,
  Annotation,
  AnnotationId,
  AskQuestion,
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
  useFeedback,
  useDeleteFeedback,
  usePostFeedback,
  useQuestions,
  type useCommentActions,
} from "../../api/queries";
import { questionsAt, subjectOf } from "../../components/ask/chat/model";
import { QuestionsHere } from "../../components/ask/chat/QuestionsHere";
import {
  CommentComposer,
  type CommentDrafts,
  type UnresolvedPosts,
} from "../../components/comment/CommentComposer";
import { commentPlacement, isChangedLine } from "../../components/comment/placement";
import { FeedbackButtons } from "../../components/feedback/FeedbackButtons";
import { githubLineUrl } from "../../components/github/links";
import {
  Inspector,
  type InspectorBack,
  type InspectorPanel,
  type InspectorProps,
} from "../../components/inspector/Inspector";
import type { SectionControl } from "../../components/inspector/sectionState";
import { StatusMessage } from "../../components/status/StatusMessage";
import { useAskChat } from "../review/askChat";
import { DiffCode } from "./code";
import { stepQuery } from "./sequence";

/** The line a comment is being written on, with the displayed lines it was chosen in. */
export interface CommentLineState {
  target: CommentTarget;
  lines: LinesResponse;
}

interface InspectorPaneProps {
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
  /** Open / collapsed Inspector sections. */
  sections: SectionControl;
  commentRef: RefObject<HTMLTextAreaElement | null>;
  /** "Ask about this line" from a code line of the step. */
  onAskLine?: (target: CommentTarget) => void;
  onAskRemark?: (annotation: Annotation) => void;
  /** "Use as review comment" on a flag of the step; absent while commenting is unavailable. */
  onFlagAsComment?: (flag: Annotation) => void;
  /** Draft text for the comment composer ("Use as review comment"); `onCommentPrefilled` once it is in. */
  commentPrefill: { text: string } | null;
  onCommentPrefilled: () => void;
  /** The Files view: its own panel instead of the block's (the block then names a narrowed step). */
  files?: FilesInspector;
}

/** What the Files view shows in the Inspector and its dock. */
export interface FilesInspector {
  /** The file, hunk or selection panel; none while a step is narrowed (the block's step panel shows). */
  panel?: InspectorPanel;
  back?: InspectorBack;
  /** The comment section's text while no line is chosen. */
  commentHint?: string;
  /** The step feedback rates; null with the reason feedback is not offered here. */
  feedback: { target: FeedbackTarget; subject: string } | { none: string };
  /** Classic diff of the hunk or selection (section of the panel). */
  diff?: ReactNode;
}

/** GitHub link to a line at the analysed commit of its side (offered only outside the PR diff). */
const permalink = (tour: Tour, target: CommentTarget): string =>
  githubLineUrl(
    tour.analysis.pr,
    { path: target.path, side: target.side, start: target.line, end: target.line },
    false,
  );

/** The inspector with its data: classic diff, the copy of the Ask chat here, and the dock (feedback, comment). */
export function InspectorPane(props: InspectorPaneProps) {
  const { analysisId, tour, block, step, annotationId, pending, comments, commentLine } = props;
  const questions = useQuestions(analysisId);
  const chat = useAskChat();
  const files = props.files;
  const feedback = useFeedback(analysisId);
  const postFeedback = usePostFeedback(analysisId);
  const deleteFeedback = useDeleteFeedback(analysisId);
  const annotation = step?.llm.annotations.find((a) => a.id === annotationId);
  // Feedback on a plain line only where the server takes it: a changed line of the diff on its side.
  const line =
    commentLine && isChangedLine(commentLine.lines, commentLine.target) ? commentLine.target : null;
  const target: FeedbackTarget | null = files
    ? "target" in files.feedback && !annotation
      ? files.feedback.target
      : annotation
        ? { target: "annotation", annotation_id: annotation.id }
        : null
    : annotation
      ? { target: "annotation", annotation_id: annotation.id }
      : line
        ? { target: "line", path: line.path, side: line.side, line: line.line }
        : step
          ? { target: "step", step_id: step.id }
          : null;

  const diff =
    files?.diff ??
    (step && (
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
    ));
  // The read-only copy of the chat's threads about where the reader is (proposal AC11).
  const place = chat.place;
  const here =
    place &&
    (questions.isError ? (
      <StatusMessage kind="error">Cannot read the questions: {errorText(questions.error)}</StatusMessage>
    ) : questions.isPending ? (
      <StatusMessage kind="loading">Loading the questions…</StatusMessage>
    ) : (
      <QuestionsHere
        threads={questionsAt(
          (questions.data as AskQuestion[]).filter((q) => q.mine !== false),
          place,
          tour,
        )}
        what={subjectOf(place, tour).what}
        onOpenInChat={(thread) => chat.open({ thread })}
        // Selected lines have their own Ask button in the Files panel.
        onAsk={
          (place.kind === "lines" && place.range) || (place.kind === "step" && place.lines)
            ? undefined
            : () => chat.open()
        }
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
      emptyHint={props.files?.commentHint}
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
      onAskRemark={props.onAskRemark}
      questions={here}
      tour={tour}
      block={block}
      panel={files?.panel}
      back={files?.back}
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
          {files && "none" in files.feedback && !annotation ? (
            <section aria-label="Feedback">
              <h3 className="m-0 text-[10.5px] font-bold tracking-wide text-muted uppercase">Feedback</h3>
              <p className="text-[12.5px] text-muted">{files.feedback.none}</p>
            </section>
          ) : (
            <FeedbackButtons
              target={target}
              subject={
                annotation
                  ? "this annotation"
                  : files && "subject" in files.feedback
                    ? files.feedback.subject
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
          )}
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
