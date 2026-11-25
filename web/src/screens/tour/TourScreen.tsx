import { AnalysisStateBanner } from "../../components/progress/AnalysisStateBanner";
import { FlowWork } from "../../components/progress/FlowWork";
import { isIdle } from "../../components/progress/idle";
import { RemarkPill } from "../../components/remark-pills/RemarkPills";
import { FlagMarker } from "../../components/flag/FlagMarker";
import { REVIEW_START_PATH } from "../shell/paths";
import { flagComment } from "../../components/comment/prefill";
import { flagTarget, remarkTarget, summariseRemarks } from "../../components/remark-pills/remarks";
import { useSectionState } from "../../components/inspector/sectionState";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Navigate, useParams, useLocation, useNavigate, useNavigationType } from "react-router";
import { ApiError } from "../../api/client";
import type {
  AnalysisId,
  Annotation,
  AnnotationId,
  AskQuestion,
  BlockRef,
  CommentTarget,
  Evidence,
  LinesResponse,
  PendingComment,
  Step,
  StepId,
  Tour,
} from "../../api/generated/types";
import {
  useCommentActions,
  useFiles,
  useLines,
  useMe,
  usePendingReview,
  useTour,
  useFeedback,
  useFlowWork,
  useForgetReanalysedFlows,
  useQuestions,
  linesQuery,
} from "../../api/queries";
import { useQueryClient } from "@tanstack/react-query";
import { useReviewStateStore } from "../../api/review-state";
import { BlockNav } from "../../components/block-nav/BlockNav";
import type { MarkerTarget } from "../../components/code-view/markers";
import { hiddenLines } from "../../components/code-view/visibility";
import { FlowIntroBlock } from "../../components/flow-intro/FlowIntroBlock";
import { isFlowReviewed } from "../../components/flow-intro/progress";
import { HelpDialog } from "../../components/help/HelpDialog";
import { quietTitle } from "../../components/inspector/summaries";
import { shortcutAction } from "../../components/help/shortcuts";
import { FlowWhy } from "../../components/flow-intro/FlowWhy";
import { NavList } from "../../components/nav/NavList";
import { filesHash, stepFilesLink } from "../../components/files/model";
import { PrContextBlock } from "../../components/pr-context/PrContextBlock";
import { MovesBlock } from "../../components/quiet/MovesBlock";
import { NotAnalysedBlock } from "../../components/quiet/NotAnalysedBlock";
import { PendingReviewBlock } from "../../components/quiet/PendingReviewBlock";
import { UnexplainedBlock } from "../../components/quiet/UnexplainedBlock";
import { StatusMessage } from "../../components/status/StatusMessage";
import { StepBlock } from "../../components/step/StepBlock";
import { ReviewTopBar } from "../shell/ReviewTopBar";
import { Tooltip } from "../../components/tooltip/Tooltip";
import { busyTip } from "../../components/button/button";
import {
  ChunkedCode,
  DiffCode,
  FileDiffCode,
  errorText,
  useHideContextChanges,
  type CodeContext,
} from "./code";
import { DiagramPane } from "./DiagramPane";
import { useCommentDrafts, useUnresolvedPosts } from "./drafts";
import { InspectorPane, type CommentLineState } from "./InspectorPane";
import { loadLayout, saveLayout, showColumn, type TourLayout } from "./layout";
import {
  blockTitle,
  buildSequence,
  findStep,
  flowJump,
  sameBlock,
  stepQuery,
  blockFragment,
  blockFromHash,
  evidenceFromHash,
} from "./sequence";
import { commentsPath } from "../pr/paths";
import { useNow } from "../../app/now";
import { FlowsMenu } from "./FlowsMenu";
import { Workspace } from "./Workspace";
import { useAskChat, useAskPlace, type AnswerDraft } from "../review/askChat";
import { StepCode } from "./StepCode";
import {
  checksHref,
  isTyping,
  openGithubLine,
  pendingCountOf,
  transient,
  useNoPageScroll,
  useVisibleCodePath,
} from "./review";

const PR_CONTEXT: BlockRef = { block: "pr_context" };

const onStep = (c: PendingComment, step: Step) =>
  c.path === step.facts.file &&
  c.line !== null &&
  (c.side ?? step.facts.side) === step.facts.side &&
  c.line >= step.facts.range.start_line &&
  (c.start_line ?? c.line) <= step.facts.range.end_line;

/** The review screen (brief §4.3): top bar, three columns, one block at a time. */
export function TourScreen() {
  const { analysisId = "" } = useParams();
  const me = useMe();
  const tour = useTour(analysisId);
  const review = useReviewStateStore(analysisId);
  const login = me.data?.user?.login ?? null;

  if (tour.error instanceof ApiError && tour.error.code === "analysis_not_finished") {
    return <Navigate to={`/analyses/${encodeURIComponent(analysisId)}/progress`} replace />;
  }
  // A transient failure of a background refetch keeps the last good tour (and the drafts and state of the open
  // review); an answer such as 401 or 404 does not change on retry.
  if (tour.isError && (!tour.data || !transient(tour.error))) {
    return (
      <Shell login={login}>
        <StatusMessage kind="error">{errorText(tour.error)}</StatusMessage>
        <a href="/">My work</a>
      </Shell>
    );
  }
  if (tour.isPending || review.status === "loading") {
    return (
      <Shell login={login}>
        <StatusMessage kind="loading">Loading the review…</StatusMessage>
      </Shell>
    );
  }
  // Session state (block, comment line, unconfirmed posts) belongs to one analysis.
  return (
    <TourView
      key={analysisId}
      analysisId={analysisId}
      tour={tour.data}
      review={review}
      login={login}
      refresh={
        tour.isError
          ? { error: errorText(tour.error), busy: tour.isFetching, retry: () => void tour.refetch() }
          : undefined
      }
    />
  );
}

interface TourViewProps {
  analysisId: AnalysisId;
  tour: Tour;
  /** Ticks and last viewed block of this analysis. */
  review: ReturnType<typeof useReviewStateStore>;
  login: string | null;
  /** The last background refresh of the tour failed; the data shown is the last good one. */
  refresh?: { error: string; busy: boolean; retry: () => void };
}

/** Where a resumed review opens, e.g. "flow 1, step 5". */
function resumedAt(tour: Tour, block: BlockRef): string {
  switch (block.block) {
    case "step": {
      const found = findStep(tour, block.step_id);
      return found ? `flow ${found.flow.position}, step ${found.step.number}` : "a step";
    }
    case "flow_intro": {
      const flow = tour.flows.find((f) => f.id === block.flow_id);
      return flow ? `the introduction of flow ${flow.position}` : "a flow introduction";
    }
    case "quiet":
      return quietTitle(block.row);
    case "pending_review":
      return "your pending review";
    case "pr_context":
      return "PR context";
  }
}

function TourView({ analysisId, tour, review, login, refresh }: TourViewProps) {
  const [hideContextChanges, changeContextVisibility] = useHideContextChanges(login);
  const { state, change: save, loadError, saveError } = review;
  const work = useFlowWork(analysisId);
  useForgetReanalysedFlows(analysisId, tour);
  const questions = useQuestions(analysisId);
  const chat = useAskChat();
  const now = useNow();
  const workAllowed = ["planning", "running", "paused"].includes(tour.analysis.state.state);
  // A finished analysis takes a request for any flow shown failed (a stored flow today's checks reject, or one
  // that failed for good); the request analyses it again from scratch.
  const finished = tour.analysis.state.state === "finished";
  const pending = usePendingReview(analysisId);
  // The GitHub Viewed marks shown in step headers (read-only here).
  const files = useFiles(analysisId);
  // Remarks the user gave feedback on dim their pills; nothing else is per-user state of a remark.
  const feedback = useFeedback(analysisId);
  const handledRemarks = new Set(
    (feedback.data ?? []).flatMap((f) => (f.target.target === "annotation" ? [f.target.annotation_id] : [])),
  );
  // Said by `r` when no step has remarks.
  const [remarkNotice, setRemarkNotice] = useState<{ text: string; nonce: number } | null>(null);
  const comments = useCommentActions(analysisId);
  const ticked = state.ticked_steps;

  const sequence = useMemo(() => buildSequence(tour), [tour]);
  const location = useLocation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const navigationType = useNavigationType();
  const [locationKey, setLocationKey] = useState(location.key);
  const known = (last: BlockRef | null | undefined): last is BlockRef =>
    !!last &&
    (sequence.some((b) => sameBlock(b, last)) || last.block === "quiet" || last.block === "pending_review");
  const [resumeBlock] = useState<BlockRef>(() => (known(state.last_viewed) ? state.last_viewed : PR_CONTEXT));
  const lastViewed = state.last_viewed;
  const current = useMemo(
    () => blockFromHash(tour, location.hash) ?? (location.hash ? PR_CONTEXT : resumeBlock),
    [tour, location.hash, resumeBlock],
  );
  const evidence = useMemo(() => evidenceFromHash(location.hash), [location.hash]);
  const lineView = location.hash.startsWith("#lines?") ? evidence : undefined;
  // A review opened elsewhere than at its start says once where it resumed (mockup "returning to a review").
  const [resumed, setResumed] = useState<BlockRef | null>(() =>
    location.hash || current.block === "pr_context" ? null : current,
  );
  const [annotationId, setAnnotationId] = useState<AnnotationId | null>(null);
  // "Show in Inspector": the remark or thread the Inspector scrolls to and highlights, until it was shown once.
  const [inspectorFlash, setInspectorFlash] = useState<(MarkerTarget & { nonce: number }) | null>(null);
  const flashNonce = useRef(0);
  const [oldCode, setOldCode] = useState(false);
  const [commentLine, setCommentLine] = useState<CommentLineState | null>(null);
  // A pending comment opened from the pending-review list, shown open at its line.
  const [revealComment, setRevealComment] = useState<string | null>(null);
  const [unresolved, setUnresolved] = useUnresolvedPosts(analysisId, login);
  const drafts = useCommentDrafts(analysisId, login);
  const [helpOpen, setHelpOpen] = useState(false);
  const [layout, setLayoutState] = useState(loadLayout);
  const [focusNext, setFocusNext] = useState<"comment" | null>(null);
  const mainRef = useRef<HTMLDivElement>(null);
  // Inspector sections, remembered per user; scoped reveals close again on another block.
  const sections = useSectionState(JSON.stringify(current));
  const commentRef = useRef<HTMLTextAreaElement>(null);

  const setLayout = (next: TourLayout) => {
    saveLayout(next);
    setLayoutState(next);
  };
  const openInspector = () => {
    if (layout.full !== "inspector" && (layout.full !== null || layout.hidden.inspector)) {
      setLayout(showColumn(layout, "inspector"));
    }
  };

  const index = sequence.findIndex((b) => sameBlock(b, current));
  const previous = index > 0 ? sequence[index - 1] : undefined;
  const next = index >= 0 ? sequence[index + 1] : undefined;
  const found = current.block === "step" ? findStep(tour, current.step_id) : undefined;
  const step = found?.step;
  const currentPath = useVisibleCodePath(
    mainRef,
    location.key,
    layout.full === "main" || (layout.full === null && !layout.hidden.main),
    lineView?.file ?? step?.facts.file,
  );
  const flow =
    found?.flow ??
    (current.block === "flow_intro" ? tour.flows.find((f) => f.id === current.flow_id) : undefined);
  const stepLines = useLines(analysisId, step && stepQuery(step));
  // The Files view at the current place: a step's lines, a flow's first step, else its first file.
  const filesPath = `/analyses/${analysisId}/files`;
  const filesStep = step ?? (current.block === "flow_intro" ? flow?.steps[0] : undefined);
  const filesHref = filesStep ? `${filesPath}${filesHash(stepFilesLink(filesStep))}` : filesPath;
  // Lines of the step the code view hides ("hide comment and import changes"); `c` and the reveal skip them.
  const stepHidden =
    step && stepLines.data
      ? hiddenLines(stepLines.data, hideContextChanges, {
          annotations: step.llm.annotations,
          threads: tour.threads,
          comments: pending.data?.comments,
        }).rows
      : new Set<number>();
  const firstShownChange = stepLines.data?.changed.find((line) => !stepHidden.has(line));
  const offersOldCode =
    step !== undefined && (step.facts.changed_lines.length > 0 || step.facts.side === "base");

  const openFragment = (hash: string) => {
    void navigate({ ...location, hash }, { flushSync: true });
    setResumed(null);
    setAnnotationId(null);
    setOldCode(false);
    setCommentLine(null);
    setRevealComment(null);
    mainRef.current?.scrollTo?.({ top: 0 });
  };
  const go = (block: BlockRef | undefined) => {
    if (!block) return;
    openFragment(blockFragment(block));
  };

  // Next / `j`: on a step it first ticks the step (never unticks), then moves on.
  const tickedOnNext = lineView ? undefined : step;
  const goNext = () => {
    if (tickedOnNext && !ticked.includes(tickedOnNext.id)) setTicks([tickedOnNext.id], true);
    go(next);
  };

  // `owner`: the step the line belongs to, when known (a remark); else the first step covering the line.
  const showEvidence = (item: Pick<Evidence, "file" | "side" | "line">, owner?: StepId) => {
    const found = tour.flows
      .flatMap((f) => f.steps)
      .find(
        (s) =>
          (owner === undefined || s.id === owner) &&
          s.facts.file === item.file &&
          s.facts.side === item.side &&
          item.line >= s.facts.range.start_line &&
          item.line <= s.facts.range.end_line,
      );
    const target: BlockRef = found ? { block: "step", step_id: found.id } : PR_CONTEXT;
    setResumed(null);
    setAnnotationId(null);
    setOldCode(false);
    setCommentLine(null);
    setRevealComment(null);
    mainRef.current?.scrollTo?.({ top: 0 });
    const query = new URLSearchParams({ file: item.file, side: item.side, line: String(item.line) });
    void navigate(
      { ...location, hash: `${found ? blockFragment(target) : "lines"}?${query}` },
      { flushSync: true },
    );
  };

  // Browser history changes bypass go(); discard controls belonging to the old block before rendering.
  if (locationKey !== location.key) {
    setLocationKey(location.key);
    if (navigationType === "POP") {
      setResumed(null);
      setAnnotationId(null);
      setOldCode(false);
      setCommentLine(null);
      setRevealComment(null);
    }
  }

  useNoPageScroll();

  // Make a resumed position immediately shareable; the server remains the fallback without a fragment.
  useEffect(() => {
    if (!location.hash) void navigate({ ...location, hash: blockFragment(current) }, { replace: true });
  }, [current, location, navigate]);

  // The last viewed block is part of the stored review state (resume).
  useEffect(() => {
    if (!lastViewed || !sameBlock(lastViewed, current)) save((s) => ({ ...s, last_viewed: current }));
  }, [current, lastViewed, save]);

  useEffect(() => {
    const field = focusNext === "comment" ? commentRef.current : null;
    if (!field) return;
    field.focus();
    setFocusNext(null);
  }, [focusNext, layout]);

  const setTicks = (steps: StepId[], on: boolean) =>
    save((s) => ({
      ...s,
      ticked_steps: on
        ? [...s.ticked_steps, ...steps.filter((id) => !s.ticked_steps.includes(id))]
        : s.ticked_steps.filter((id) => !steps.includes(id)),
    }));

  const comment = (target: CommentTarget, lines: LinesResponse) => {
    setCommentLine({ target, lines });
    openInspector();
  };
  // The open comment's line, else the step's first changed line: where `c` and an Ask answer's draft go.
  const currentLine = (): CommentLineState | null => {
    if (commentLine) return commentLine;
    if (!step || !stepLines.data) return null;
    const { from, lines } = stepLines.data;
    const line = firstShownChange ?? lines.map((_, i) => from + i).find((l) => !stepHidden.has(l));
    if (line === undefined) return null;
    return {
      target: { path: step.facts.file, side: step.facts.side, start_line: null, line },
      lines: stepLines.data,
    };
  };
  // `c`: back to the open comment, else a comment on the step's first changed line.
  const commentOnCurrentLine = () => {
    if (commentLine) {
      openInspector();
      setFocusNext("comment");
      return;
    }
    const at = currentLine();
    if (at) comment(at.target, at.lines);
  };
  // "Use as review comment": the composer opens on `at` with `text` as a draft; nothing is sent.
  const [commentPrefill, setCommentPrefill] = useState<{ text: string } | null>(null);
  const draftComment = (at: CommentLineState | null, text: string) => {
    if (!at) return;
    comment(at.target, at.lines);
    setCommentPrefill({ text });
  };
  const commenting = pending.data?.commenting_available === true;
  const onFlagAsComment =
    commenting && step && stepLines.data
      ? (flag: Annotation) => {
          const { start_line, end_line } = flag.range;
          const target = {
            path: step.facts.file,
            side: step.facts.side,
            start_line: start_line === end_line ? null : start_line,
            line: end_line,
          };
          draftComment(stepLines.data ? { target, lines: stepLines.data } : null, flagComment(flag));
        }
      : undefined;
  // LLM remark pills and `r`: the remark's code line and its Inspector entry flash.
  // `flag`: the first flag at the strongest level (a flag marker); else the first remark (a rating pill, `r`).
  const jumpToRemark = (steps: Step[], target: "remark" | "flag" = "remark") => {
    const remark = target === "flag" ? flagTarget(steps) : remarkTarget(steps, handledRemarks);
    if (!remark) return;
    const { step: owner, annotation } = remark;
    if (!annotation) return go({ block: "step", step_id: owner.id });
    showEvidence(
      { file: owner.facts.file, side: owner.facts.side, line: annotation.range.start_line },
      owner.id,
    );
    flashNonce.current += 1;
    setInspectorFlash({ kind: "annotation", id: annotation.id, nonce: flashNonce.current });
    openInspector();
  };
  const showInInspector = (target: MarkerTarget) => {
    const owner = target.kind === "thread" ? tour.threads.find((t) => t.id === target.id)?.step_id : null;
    if (owner && owner !== step?.id) go({ block: "step", step_id: owner });
    else if (!(target.kind === "annotation" && target.id === annotationId)) setAnnotationId(null);
    flashNonce.current += 1;
    setInspectorFlash({ ...target, nonce: flashNonce.current });
    openInspector();
  };

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || helpOpen || isTyping(event)) return;
      switch (shortcutAction(event.key)) {
        case "next_block":
          return goNext();
        case "previous_block":
          return go(previous);
        case "next_flow":
          return go(flowJump(tour, current, 1));
        case "previous_flow":
          return go(flowJump(tour, current, -1));
        case "tick_step":
          if (step) setTicks([step.id], !ticked.includes(step.id));
          return;
        case "toggle_old_code":
          if (offersOldCode) setOldCode((shown) => !shown);
          return;
        case "github_line":
          event.preventDefault();
          return openGithubLine(mainRef.current, currentLine()?.target ?? lineView);
        case "comment":
          event.preventDefault();
          return commentOnCurrentLine();
        case "next_remark": {
          // After the current step; on a flow introduction from that flow's first step; then around.
          const order = tour.flows.flatMap((f) => f.steps);
          const from =
            step && current.block === "step"
              ? order.findIndex((s) => s.id === step.id) + 1
              : current.block === "flow_intro" && flow
                ? tour.flows.slice(0, tour.flows.indexOf(flow)).reduce((n, f) => n + f.steps.length, 0)
                : 0;
          const start = Math.max(0, from);
          const next = [...order.slice(start), ...order.slice(0, start)].find(
            (s) => s.llm.annotations.length > 0,
          );
          if (next) jumpToRemark([next]);
          else setRemarkNotice((n) => ({ text: "No more steps with remarks", nonce: (n?.nonce ?? 0) + 1 }));
          return;
        }
        case "ask":
          event.preventDefault();
          return chat.open();
        case "switch_view":
          return void navigate(filesHref);
        case "help":
          return setHelpOpen(true);
        case "close":
          if (commentLine) setCommentLine(null);
          else if (annotationId) setAnnotationId(null);
          else if (layout.full) setLayout({ ...layout, full: null });
          return;
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  });

  // The Ask chat's subject follows the block: a step, a flow introduction, the PR context (proposal AC8).
  useAskPlace(
    lineView
      ? null
      : current.block === "step"
        ? { kind: "step", step_id: current.step_id, lines: null }
        : // A flow not analysed yet cannot be asked about (the server says 404): the whole PR instead.
          current.block === "flow_intro" &&
            (!flow?.analysis_status || flow.analysis_status.state === "analysed")
          ? { kind: "flow", flow_id: current.flow_id }
          : current.block === "pr_context" || current.block === "flow_intro"
            ? { kind: "pr" }
            : null,
  );
  // An answer's evidence opens here; "Use as review comment" drafts on its lines and focuses the composer (AC12).
  // An answer about another step or lines of a file opens that place first, says so, and drafts there.
  useEffect(() => {
    chat.setActions({
      onEvidence: showEvidence,
      onUseAsComment: (anchor, text) => {
        // Lines of a file: Files drafts (and says so) once it has their lines.
        if (anchor.kind === "lines" && anchor.range) {
          const { path, side, range } = anchor;
          const hash = filesHash({ file: path, side, line: range.start_line, end: range.end_line });
          return void navigate(`${filesPath}${hash}`, {
            state: { commentDraft: { path, side, range, text } satisfies AnswerDraft },
          });
        }
        if (anchor.kind !== "step") return;
        const found = findStep(tour, anchor.step_id);
        if (!found) return;
        const target = found.step;
        const draftOn = (lines: LinesResponse, first: number | undefined) => {
          const at = anchor.lines ?? (first === undefined ? null : { start_line: first, end_line: first });
          if (!at) return;
          const where = { path: target.facts.file, side: target.facts.side };
          const start_line = at.start_line === at.end_line ? null : at.start_line;
          draftComment({ target: { ...where, start_line, line: at.end_line }, lines }, text);
          setFocusNext("comment");
        };
        if (step && target.id === step.id && stepLines.data) {
          if (!anchor.lines && commentLine) draftComment(commentLine, text);
          else draftOn(stepLines.data, currentLine()?.target.line);
          return setFocusNext("comment");
        }
        go({ block: "step", step_id: target.id });
        queryClient.fetchQuery(linesQuery(analysisId, stepQuery(target))).then(
          (lines) => {
            draftOn(lines, lines.changed[0] ?? lines.from);
            chat.announce(
              `Opened flow ${found.flow.position} · step ${target.number} to comment on its lines; the draft is in the Comment composer.`,
            );
          },
          (error: unknown) => chat.announce(`The answer's lines could not be read: ${errorText(error)}`),
        );
      },
    });
  });
  const pendingComments = pending.data?.comments ?? [];
  const pendingCount = pendingCountOf(pending);
  const askRemark = (remark: Annotation) => {
    const owner = tour.flows
      .flatMap((flow) => flow.steps)
      .find((s) => s.llm.annotations.some((annotation) => annotation.id === remark.id));
    if (owner) chat.open({ anchor: { kind: "step", step_id: owner.id, lines: remark.range }, remark });
  };
  const ctx: CodeContext = {
    hideContextChanges,
    analysisId,
    pr: tour.analysis.pr,
    files: files.data?.files,
    threads: tour.threads,
    comments: pendingComments,
    activeLine: commentLine?.target ?? null,
    onComment: comment,
    // "Ask about this line": the chat opens about the line (or the selected lines); nothing is sent.
    // A step anchor only for lines inside the step (the server refuses others); else lines of the file.
    onAskLine: step
      ? (target) => {
          const range = { start_line: target.start_line ?? target.line, end_line: target.line };
          const inside =
            target.side === step.facts.side &&
            target.path === step.facts.file &&
            step.facts.range.start_line <= range.start_line &&
            range.end_line <= step.facts.range.end_line;
          chat.open({
            anchor: inside
              ? { kind: "step", step_id: step.id, lines: range }
              : { kind: "lines", path: target.path, side: target.side, range },
          });
        }
      : undefined,
    revealComment,
    onEditComment: comments.edit,
    onDeleteComment: comments.remove,
    flash: evidence ?? null,
    onShowInInspector: showInInspector,
    onAskRemark: askRemark,
    otherThreadStep: (t) => {
      const owner = t.step_id && t.step_id !== step?.id ? findStep(tour, t.step_id) : undefined;
      return owner ? `${owner.flow.position}.${owner.step.number}` : null;
    },
  };
  // The step that shows a pending comment's line; a file comment without a shown line: the first step of its file.
  const placeOf = (c: PendingComment) => {
    const all = tour.flows.flatMap((f) => f.steps.map((s) => ({ flow: f, step: s })));
    const at =
      all.find(({ step }) => onStep(c, step)) ??
      (c.kind === "file" ? all.find(({ step }) => step.facts.file === c.path) : undefined);
    if (!at) return null;
    return {
      label: `step ${at.flow.position}.${at.step.number}`,
      open: () => {
        go({ block: "step", step_id: at.step.id });
        setRevealComment(c.id);
      },
    };
  };
  const selectStep = (id: StepId) => go({ block: "step", step_id: id });
  // The user's own questions in tour order (GET …/questions lists only the signed-in user's).
  const stepOrder = tour.flows.flatMap((f) => f.steps.map((s) => s.id));
  const rank = (id: StepId) => (stepOrder.includes(id) ? stepOrder.indexOf(id) : stepOrder.length);
  const myQuestions = (questions.data ?? [])
    // Questions about lines (Files view) have no step and are not part of the tour.
    .filter((q): q is AskQuestion & { step_id: StepId } => q.mine !== false && q.step_id !== undefined)
    .sort((a, b) => rank(a.step_id) - rank(b.step_id));
  const questionCounts: Record<StepId, number> = {};
  for (const q of myQuestions) questionCounts[q.step_id] = (questionCounts[q.step_id] ?? 0) + 1;

  let block: ReactNode;
  if (lineView) {
    block = (
      <section aria-label="Evidence code">
        <h2>
          Code: {lineView.file}:{lineView.line}
        </h2>
        <ChunkedCode
          key={location.hash}
          ctx={ctx}
          path={lineView.file}
          side={lineView.side}
          ranges={[{ start_line: Math.max(1, lineView.line - 3), end_line: lineView.line + 6 }]}
        />
      </section>
    );
  } else if (current.block === "flow_intro" && flow) {
    block = (
      <>
        <div className="flex flex-wrap items-center gap-2 empty:hidden">
          <RemarkPill
            summary={summariseRemarks(flow.steps, handledRemarks)}
            scope="flow"
            subject="this flow"
            withRating={false}
            label="LLM remarks:"
            onSelect={() => jumpToRemark(flow.steps)}
          />
          <FlagMarker
            annotations={flow.steps.flatMap((s) => s.llm.annotations)}
            scope="flow"
            subject="this flow"
            onSelect={() => jumpToRemark(flow.steps, "flag")}
          />
        </div>
        <FlowIntroBlock
          flow={flow}
          onAsk={() => chat.open()}
          analysis={tour.analysis.state}
          now={now}
          onRequest={
            workAllowed || (finished && flow.analysis_status?.state === "failed")
              ? () => work.request.mutate(flow.id)
              : undefined
          }
          busy={work.request.isPending}
          error={
            work.request.error && work.request.variables === flow.id
              ? errorText(work.request.error)
              : undefined
          }
          dependencies={tour.flows.filter((f) => flow.llm.depends_on.includes(f.id))}
          ticked={ticked}
          onSetReviewed={(on) =>
            setTicks(
              flow.steps.map((s) => s.id),
              on,
            )
          }
        />
      </>
    );
  } else if (step && found) {
    block = (
      <StepBlock
        key={step.id}
        onAsk={() => chat.open()}
        step={step}
        pr={tour.analysis.pr}
        stepOf={(id) => findStep(tour, id)?.step}
        threadsStatus={tour.pr_context.facts.threads_status}
        code={
          <>
            {/* Apart from the remark label at the top of the code, so both keep a 24 px target. */}
            <div className="mb-2 flex flex-wrap items-center gap-2">
              <RemarkPill
                summary={summariseRemarks([step], handledRemarks)}
                scope="step"
                subject="this step"
                label="LLM rating and remarks:"
                onSelect={() => jumpToRemark([step])}
              />
              <FlagMarker
                annotations={step.llm.annotations}
                scope="step"
                subject="this step"
                onSelect={() => jumpToRemark([step], "flag")}
              />
            </div>
            {oldCode ? (
              <DiffCode analysisId={analysisId} query={stepQuery(step)} comments={ctx} />
            ) : (
              <StepCode
                key={location.key}
                step={step}
                siblings={found.flow.steps.map((other) => ({
                  step: other,
                  label: `Step ${found.flow.position}.${other.number}`,
                  href: `#${blockFragment({ block: "step", step_id: other.id })}`,
                }))}
                onStep={(id) => void go({ block: "step", step_id: id })}
                ctx={ctx}
                revealLine={evidence?.line ?? firstShownChange}
                lines={stepLines}
                annotations={step.llm.annotations}
                band={step.llm.assessment?.risk ?? null}
                kind={step.llm.kind}
                selectedAnnotation={annotationId}
                onSelectAnnotation={(ids) => {
                  setAnnotationId(ids[0] ?? null);
                  openInspector();
                }}
              />
            )}
          </>
        }
        patterns={found.flow.llm.pattern_repetitions}
        impactCode={(impact) => (
          <ChunkedCode ctx={ctx} path={impact.file} side="head" ranges={[impact.range]} />
        )}
        reviewed={ticked.includes(step.id)}
        onSetReviewed={(on) => setTicks([step.id], on)}
        ownComments={pendingComments.filter((c) => onStep(c, step)).length}
        oldCode={offersOldCode ? { shown: oldCode, onToggle: () => setOldCode((s) => !s) } : undefined}
        onSelectStep={selectStep}
        viewed={
          // null: GitHub's mark was not read (said so); undefined: the file list is not known (no chip).
          files.data?.files.find((f) => f.path === step.facts.file || f.old_path === step.facts.file)?.viewed
        }
        onOpenFile={() => void navigate(filesHref)}
      />
    );
  } else if (current.block === "quiet") {
    const row = current.row;
    if (row.row === "unexplained") {
      block = (
        <UnexplainedBlock
          items={tour.quiet.unexplained}
          code={(item) => <ChunkedCode ctx={ctx} path={item.file} side={item.side} ranges={item.ranges} />}
        />
      );
    } else if (row.row === "moves") {
      block = (
        <MovesBlock
          moves={tour.quiet.moves}
          code={(move) => (
            <ChunkedCode ctx={ctx} path={move.to.file} side={move.to.side} ranges={[move.to.range]} />
          )}
        />
      );
    } else {
      const section = tour.quiet.not_analysed.find((s) => s.category === row.category);
      block = section && (
        <NotAnalysedBlock
          key={section.category}
          section={section}
          counters={tour.pr_context.facts.counters}
          code={(item, range) =>
            section.category === "tests" && item.ranges === null ? (
              <FileDiffCode ctx={ctx} path={item.file} side={item.side} />
            ) : section.category === "tests" ? (
              <DiffCode
                analysisId={analysisId}
                query={{ path: item.file, side: item.side, from: range.start_line, to: range.end_line }}
                comments={ctx}
              />
            ) : (
              <ChunkedCode ctx={ctx} path={item.file} side={item.side} ranges={[range]} />
            )
          }
        />
      );
    }
  } else if (current.block === "pending_review") {
    block = pending.isSuccess ? (
      <PendingReviewBlock
        review={pending.data}
        prUrl={tour.analysis.pr.html_url}
        onEdit={comments.edit}
        onDelete={comments.remove}
        placeOf={placeOf}
      />
    ) : pending.isError ? (
      <StatusMessage kind="error">Cannot read your pending review: {errorText(pending.error)}</StatusMessage>
    ) : (
      <StatusMessage kind="loading">Loading your pending review…</StatusMessage>
    );
  } else if (current.block === "pr_context") {
    block = (
      <PrContextBlock
        onAsk={() => chat.open()}
        usage={tour.analysis.usage}
        modelsUsed={tour.analysis.models_used}
        onEvidence={showEvidence}
        pr={tour.analysis.pr}
        context={tour.pr_context}
        threadCount={tour.threads.length}
        checksHref={checksHref(tour.analysis.pr)}
      />
    );
  }

  const progressAlert =
    review.status === "unreadable"
      ? `Your ticks and last viewed block could not be read. Changes stay on this page and are saved once they can be read: ${errorText(loadError)}`
      : saveError !== null
        ? `Your ticks and last viewed block are not saved: ${errorText(saveError)}`
        : null;
  // Flows run on request only; "Analyse all remaining flows" (background_analysis) is the opt-in for the rest.
  const background = tour.analysis.background_analysis;
  const pendingFlows = tour.flows.filter((f) => f.analysis_status && f.analysis_status.state !== "analysed");
  const flowWork = tour.analysis.review_ready && workAllowed && (
    <FlowWork
      done={tour.flows.length - pendingFlows.length}
      total={tour.flows.length}
      remainingLines={pendingFlows.reduce(
        (n, f) =>
          n +
          (f.planned?.ranges ?? []).reduce(
            (m, r) => m + r.ranges.reduce((k, x) => k + x.end_line - x.start_line + 1, 0),
            0,
          ),
        0,
      )}
      background={background}
      parked={background === false && pendingFlows.length > 0 && isIdle(tour.analysis.state)}
      busy={work.background.isPending}
      onBackground={(enabled) => work.background.mutate(enabled)}
      progressHref={`/analyses/${analysisId}/progress`}
      error={work.background.error ? errorText(work.background.error) : undefined}
    />
  );
  const centre = (
    <>
      <AnalysisStateBanner
        state={tour.analysis.state}
        busy={work.resume.isPending || work.retry.isPending}
        onResume={() => work.resume.mutate()}
        onRetry={() => work.retry.mutate()}
        progressHref={`/analyses/${analysisId}/progress`}
        error={
          work.resume.error || work.retry.error ? errorText(work.resume.error || work.retry.error) : undefined
        }
      />
      {resumed && sameBlock(resumed, current) && (
        <div className="mb-2 flex items-center gap-2 rounded-md border border-border bg-surface px-3 py-1 text-[12.5px]">
          <p role="status" className="m-0 flex-1">
            Resumed where you left off: {resumedAt(tour, resumed)}.
          </p>
          <Tooltip tip="Hides this note; you stay on this block.">
            <button
              type="button"
              onClick={() => setResumed(null)}
              className="min-h-6 cursor-pointer rounded border border-border bg-surface px-1.5 text-[11.5px] hover:border-accent"
            >
              Dismiss
            </button>
          </Tooltip>
        </div>
      )}
      {refresh && (
        <div
          role="alert"
          className="mb-2 flex flex-wrap items-center gap-2 rounded-md border border-l-4 border-border border-l-med-bd bg-surface px-3 py-1 text-[12.5px]"
        >
          <p className="m-0 flex-1">
            Could not refresh the review: {refresh.error} This page shows the last version read.
          </p>
          <Tooltip
            tip={busyTip("Reads the review again; nothing you entered on this page is lost.", refresh.busy)}
          >
            <button
              type="button"
              aria-disabled={refresh.busy || undefined}
              onClick={() => !refresh.busy && refresh.retry()}
              className="min-h-6 cursor-pointer rounded border border-border bg-surface px-1.5 text-[11.5px] hover:border-accent"
            >
              Retry
            </button>
          </Tooltip>
        </div>
      )}
      {progressAlert && (
        <p
          role="alert"
          className="mb-2 rounded-md border border-l-4 border-high-bd bg-high-bg px-3 py-1.5 text-[12.5px]"
        >
          {progressAlert}
        </p>
      )}
      {step && flow && (
        <FlowWhy
          flow={flow}
          open={sections.isOpen("whyflow")}
          onToggle={() => sections.toggle("whyflow")}
          onOpenIntro={() => go({ block: "flow_intro", flow_id: flow.id })}
        />
      )}
      {flow && flow.steps.length > 0 && (
        <DiagramPane
          key={flow.id}
          flow={flow}
          currentStepId={step?.id ?? null}
          onSelectStep={selectStep}
          open={layout.diagram}
          onToggle={() => setLayout({ ...layout, diagram: !layout.diagram })}
          height={layout.diagramHeight}
          onHeight={(diagramHeight) => setLayout({ ...layout, diagramHeight })}
        />
      )}
      {block ?? <StatusMessage kind="empty">This block is not part of the analysis.</StatusMessage>}
      <BlockNav
        previous={previous && blockTitle(tour, previous)}
        next={next && blockTitle(tour, next)}
        onPrevious={() => go(previous)}
        marksReviewed={Boolean(tickedOnNext)}
        onNext={goNext}
      />
      <p className="sr-only" aria-live="polite">
        {blockTitle(tour, current)}
      </p>
      {remarkNotice && (
        <p key={remarkNotice.nonce} className="sr-only" role="status">
          {remarkNotice.text}
        </p>
      )}
    </>
  );

  return (
    // Exactly the viewport: only the columns scroll, never the document (Space, PageDown, wheel on the top bar).
    <div className="flex h-dvh flex-col overflow-hidden">
      <a
        href="#main"
        onClick={(event) => {
          event.preventDefault();
          mainRef.current?.focus();
        }}
        className="absolute -left-[999px] z-[100] bg-surface px-2.5 py-1.5 focus:top-2 focus:left-2"
      >
        Skip to content
      </a>
      <ReviewTopBar
        pr={tour.analysis.pr}
        currentPath={currentPath}
        ci={tour.pr_context.facts.ci}
        checksHref={checksHref(tour.analysis.pr)}
        language={tour.analysis.language}
        login={login}
        hideContextChanges={hideContextChanges}
        onHideContextChanges={changeContextVisibility}
        progress={{
          reviewed: tour.flows.filter((f) => isFlowReviewed(f, ticked)).length,
          total: tour.flows.length,
        }}
        progressMenu={
          <FlowsMenu
            tour={tour}
            ticked={ticked}
            work={work}
            onOpenFlow={(flowId) => go({ block: "flow_intro", flow_id: flowId })}
          />
        }
        pendingCount={pendingCount}
        onOpenPending={() => go({ block: "pending_review" })}
        onHelp={() => setHelpOpen(true)}
        view={{
          current: "flows",
          flowsHref: `${location.pathname}${location.hash}`,
          filesHref,
          depsHref: `/analyses/${analysisId}/deps`,
          commentsHref: commentsPath(tour.analysis.pr),
          prContextHref: `/analyses/${analysisId}#${blockFragment(PR_CONTEXT)}`,
          onNavigate: (href) => void navigate(href),
        }}
      />
      <Workspace
        layout={layout}
        onChange={setLayout}
        mainRef={mainRef}
        nav={
          <>
            {flowWork}
            <NavList
              handledRemarks={handledRemarks}
              onRemark={jumpToRemark}
              onFlag={(steps) => jumpToRemark(steps, "flag")}
              onSetFlowReviewed={(f, on) =>
                setTicks(
                  f.steps.map((s) => s.id),
                  on,
                )
              }
              flows={tour.flows}
              analysis={tour.analysis.state}
              quiet={tour.quiet}
              ticked={ticked}
              pendingCount={pendingCount}
              current={current}
              onSelect={go}
              questionCounts={questionCounts}
              questions={{
                count: questions.isSuccess
                  ? (questions.data ?? []).filter((q) => q.mine !== false).length
                  : questions.isError
                    ? "error"
                    : "loading",
                onOpen: () => chat.open(),
              }}
            />
          </>
        }
        main={centre}
        inspector={
          <InspectorPane
            analysisId={analysisId}
            tour={tour}
            block={current}
            step={step}
            annotationId={annotationId}
            flash={inspectorFlash}
            onFlashed={() => setInspectorFlash(null)}
            onBackToStep={() => setAnnotationId(null)}
            onSelectStep={selectStep}
            onEvidence={showEvidence}
            codeShown={
              !!step || !!lineView || (current.block === "quiet" && current.row.row !== "not_analysed")
            }
            pending={pending}
            comments={comments}
            commentLine={commentLine}
            onCancelComment={() => setCommentLine(null)}
            unresolved={unresolved}
            onUnresolvedChange={setUnresolved}
            drafts={drafts}
            reanalyseHref={`${REVIEW_START_PATH}?pr=${encodeURIComponent(tour.analysis.pr.html_url)}`}
            sections={sections}
            commentRef={commentRef}
            onAskLine={ctx.onAskLine}
            onAskRemark={askRemark}
            onFlagAsComment={onFlagAsComment}
            commentPrefill={commentPrefill}
            onCommentPrefilled={() => setCommentPrefill(null)}
          />
        }
      />
      <HelpDialog open={helpOpen} onClose={() => setHelpOpen(false)} />
    </div>
  );
}

function Shell({ login, children }: { login: string | null; children: ReactNode }) {
  return (
    <div className="flex min-h-screen flex-col">
      <ReviewTopBar login={login} />
      <main id="main" className="grid gap-3 px-8 py-5">
        {children}
      </main>
    </div>
  );
}
