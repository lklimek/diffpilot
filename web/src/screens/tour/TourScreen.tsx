import { FlowWork } from "../../components/progress/FlowWork";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Navigate, useParams, useLocation, useNavigate, useNavigationType } from "react-router";
import { ApiError } from "../../api/client";
import type {
  AnalysisId,
  AnnotationId,
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
  useLines,
  useMe,
  usePendingReview,
  useTour,
  useFlowWork,
  useQuestionEvents,
} from "../../api/queries";
import { useReviewStateStore } from "../../api/review-state";
import { BlockNav } from "../../components/block-nav/BlockNav";
import type { UnresolvedPosts } from "../../components/comment/CommentComposer";
import { FlowIntroBlock } from "../../components/flow-intro/FlowIntroBlock";
import { isFlowReviewed } from "../../components/flow-intro/progress";
import { HelpDialog } from "../../components/help/HelpDialog";
import { quietTitle } from "../../components/inspector/summaries";
import { shortcutAction } from "../../components/help/shortcuts";
import { NavList } from "../../components/nav/NavList";
import { PrContextBlock } from "../../components/pr-context/PrContextBlock";
import { MovesBlock } from "../../components/quiet/MovesBlock";
import { NotAnalysedBlock } from "../../components/quiet/NotAnalysedBlock";
import { PendingReviewBlock } from "../../components/quiet/PendingReviewBlock";
import { UnexplainedBlock } from "../../components/quiet/UnexplainedBlock";
import { StatusMessage } from "../../components/status/StatusMessage";
import { StepBlock } from "../../components/step/StepBlock";
import { TopBar } from "../../components/top-bar/TopBar";
import { Tooltip } from "../../components/tooltip/Tooltip";
import { ChunkedCode, DiffCode, FileDiffCode, LinesCode, errorText, type CodeContext } from "./code";
import { DiagramPane } from "./DiagramPane";
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
import { useNow } from "../../app/now";
import { Workspace } from "./Workspace";

const PR_CONTEXT: BlockRef = { block: "pr_context" };

// Ignore shortcuts while the user types or uses modifier keys.
function isTyping(event: KeyboardEvent): boolean {
  const target = event.target as HTMLElement | null;
  return (
    event.altKey ||
    event.ctrlKey ||
    event.metaKey ||
    target?.isContentEditable === true ||
    ["INPUT", "TEXTAREA", "SELECT"].includes(target?.tagName ?? "")
  );
}

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
  if (tour.isError) {
    return (
      <Shell login={login}>
        <StatusMessage kind="error">{errorText(tour.error)}</StatusMessage>
        <a href="/">Back to Home</a>
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
  return <TourView key={analysisId} analysisId={analysisId} tour={tour.data} review={review} login={login} />;
}

interface TourViewProps {
  analysisId: AnalysisId;
  tour: Tour;
  /** Ticks and last viewed block of this analysis. */
  review: ReturnType<typeof useReviewStateStore>;
  login: string | null;
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

function TourView({ analysisId, tour, review, login }: TourViewProps) {
  const contextKey = `smart-review.hide-context-changes.${login ?? "anonymous"}`;
  const [hideContextChanges, setHideContextChanges] = useState(() => {
    try {
      return localStorage.getItem(contextKey) !== "false";
    } catch {
      return true;
    }
  });
  const changeContextVisibility = (hide: boolean) => {
    setHideContextChanges(hide);
    try {
      localStorage.setItem(contextKey, String(hide));
    } catch {
      /* Storage unavailable; keep this view's preference. */
    }
  };
  const { state, change: save, loadError, saveError } = review;
  const work = useFlowWork(analysisId);
  const stream = useQuestionEvents(analysisId);
  const now = useNow();
  const requested = useRef<string | null>(null);
  const workAllowed = ["planning", "running", "paused"].includes(tour.analysis.state.state);
  const pending = usePendingReview(analysisId);
  const comments = useCommentActions(analysisId);
  const ticked = state.ticked_steps;

  const sequence = useMemo(() => buildSequence(tour), [tour]);
  const location = useLocation();
  const navigate = useNavigate();
  const navigationType = useNavigationType();
  const [locationKey, setLocationKey] = useState(location.key);
  const [resumeBlock] = useState<BlockRef>(() => {
    const last = state.last_viewed;
    const known =
      last &&
      (sequence.some((b) => sameBlock(b, last)) || last.block === "quiet" || last.block === "pending_review");
    return known ? last : PR_CONTEXT;
  });
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
  const [oldCode, setOldCode] = useState(false);
  const [commentLine, setCommentLine] = useState<CommentLineState | null>(null);
  // A pending comment opened from the pending-review list, shown open at its line.
  const [revealComment, setRevealComment] = useState<string | null>(null);
  const [unresolved, setUnresolved] = useState<UnresolvedPosts>({});
  const [helpOpen, setHelpOpen] = useState(false);
  const [layout, setLayoutState] = useState(loadLayout);
  const [focusNext, setFocusNext] = useState<"ask" | "comment" | null>(null);
  const mainRef = useRef<HTMLDivElement>(null);
  const askRef = useRef<HTMLTextAreaElement>(null);
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
  const flow =
    found?.flow ??
    (current.block === "flow_intro" ? tour.flows.find((f) => f.id === current.flow_id) : undefined);
  const stepLines = useLines(analysisId, step && stepQuery(step));
  const offersOldCode =
    step !== undefined && (step.facts.changed_lines.length > 0 || step.facts.side === "base");

  const go = (block: BlockRef | undefined) => {
    if (!block) return;
    void navigate({ ...location, hash: blockFragment(block) }, { flushSync: true });
    setResumed(null);
    setAnnotationId(null);
    setOldCode(false);
    setCommentLine(null);
    setRevealComment(null);
    mainRef.current?.scrollTo?.({ top: 0 });
  };

  useEffect(() => {
    const id = flow?.id ?? null;
    if (!workAllowed || requested.current === id) return;
    requested.current = id;
    if (id && (flow?.analysis_status?.state === "not_analysed" || flow?.analysis_status?.state === "queued"))
      work.request.mutate(id);
  }, [flow, work.request, workAllowed]);

  const showEvidence = (item: Evidence) => {
    const found = tour.flows
      .flatMap((f) => f.steps)
      .find(
        (s) =>
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

  // The review screen never scrolls the page: no reserved scrollbar gutter, the columns get the full width.
  useEffect(() => {
    const style = document.documentElement.style;
    const before = style.scrollbarGutter;
    style.scrollbarGutter = "auto";
    return () => {
      style.scrollbarGutter = before;
    };
  }, []);

  // Make a resumed position immediately shareable; the server remains the fallback without a fragment.
  useEffect(() => {
    if (!location.hash) void navigate({ ...location, hash: blockFragment(current) }, { replace: true });
  }, [current, location, navigate]);

  // The last viewed block is part of the stored review state (resume).
  const lastViewed = state.last_viewed;
  useEffect(() => {
    if (!lastViewed || !sameBlock(lastViewed, current)) save((s) => ({ ...s, last_viewed: current }));
  }, [current, lastViewed, save]);

  useEffect(() => {
    const field = focusNext === "ask" ? askRef.current : focusNext === "comment" ? commentRef.current : null;
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
  // `c`: back to the open comment, else a comment on the step's first changed line.
  const commentOnCurrentLine = () => {
    if (commentLine) {
      openInspector();
      setFocusNext("comment");
    } else if (step && stepLines.data) {
      const line = step.facts.changed_lines[0]?.start_line ?? step.facts.range.start_line;
      comment({ path: step.facts.file, side: step.facts.side, start_line: null, line }, stepLines.data);
    }
  };

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || helpOpen || isTyping(event)) return;
      switch (shortcutAction(event.key)) {
        case "next_block":
          return go(next);
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
        case "comment":
          event.preventDefault();
          return commentOnCurrentLine();
        case "ask":
          event.preventDefault();
          openInspector();
          return setFocusNext("ask");
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

  const pendingComments = pending.data?.comments ?? [];
  const pendingCount = pending.isSuccess
    ? pending.data.commenting_available
      ? pending.data.incomplete
        ? (`${pending.data.comments.length}+` as const)
        : pending.data.comments.length
      : ("unavailable" as const)
    : pending.isPending
      ? ("loading" as const)
      : undefined;
  const ctx: CodeContext = {
    hideContextChanges,
    analysisId,
    threads: tour.threads,
    comments: pendingComments,
    activeLine: commentLine?.target ?? null,
    onComment: comment,
    revealComment,
    onEditComment: comments.edit,
    onDeleteComment: comments.remove,
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
      <FlowIntroBlock
        flow={flow}
        now={now}
        onRequest={workAllowed ? () => work.request.mutate(flow.id) : undefined}
        busy={work.request.isPending}
        dependencies={tour.flows.filter((f) => flow.llm.depends_on.includes(f.id))}
        ticked={ticked}
        onSetReviewed={(on) =>
          setTicks(
            flow.steps.map((s) => s.id),
            on,
          )
        }
      />
    );
  } else if (step && found) {
    block = (
      <StepBlock
        key={step.id}
        step={step}
        threadsStatus={tour.pr_context.facts.threads_status}
        code={
          oldCode ? (
            <DiffCode analysisId={analysisId} query={stepQuery(step)} comments={ctx} />
          ) : (
            <LinesCode
              key={location.key}
              ctx={ctx}
              revealLine={
                evidence?.line ??
                stepLines.data?.changed.find(
                  (line) => !hideContextChanges || !stepLines.data?.context_only?.includes(line),
                )
              }
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
          )
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
        usage={tour.analysis.usage}
        modelsUsed={tour.analysis.models_used}
        onEvidence={showEvidence}
        pr={tour.analysis.pr}
        context={tour.pr_context}
        threadCount={tour.threads.length}
      />
    );
  }

  const progressAlert =
    review.status === "unreadable"
      ? `Your ticks and last viewed block could not be read. Changes stay on this page and are saved once they can be read: ${errorText(loadError)}`
      : saveError !== null
        ? `Your ticks and last viewed block are not saved: ${errorText(saveError)}`
        : null;
  const centre = (
    <>
      {tour.analysis.review_ready && workAllowed && (
        <FlowWork
          done={tour.flows.filter((f) => f.analysis_status?.state === "analysed").length}
          total={tour.flows.length}
          background={tour.analysis.background_analysis ?? true}
          busy={work.background.isPending}
          onBackground={(enabled) => work.background.mutate(enabled)}
          progressHref={`/analyses/${analysisId}/progress`}
          error={
            work.request.error || work.background.error
              ? errorText(work.request.error || work.background.error)
              : undefined
          }
        />
      )}
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
      {progressAlert && (
        <p
          role="alert"
          className="mb-2 rounded-md border border-l-4 border-high-bd bg-high-bg px-3 py-1.5 text-[12.5px]"
        >
          {progressAlert}
        </p>
      )}
      {flow && flow.steps.length > 0 && (
        <DiagramPane
          key={flow.id}
          flow={flow}
          currentStepId={step?.id ?? null}
          onSelectStep={selectStep}
          open={layout.diagram}
          onToggle={() => setLayout({ ...layout, diagram: !layout.diagram })}
        />
      )}
      {block ?? <StatusMessage kind="empty">This block is not part of the analysis.</StatusMessage>}
      <BlockNav
        previous={previous && blockTitle(tour, previous)}
        next={next && blockTitle(tour, next)}
        onPrevious={() => go(previous)}
        onNext={() => go(next)}
      />
      <p className="sr-only" aria-live="polite">
        {blockTitle(tour, current)}
      </p>
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
      <TopBar
        pr={tour.analysis.pr}
        ci={tour.pr_context.facts.ci}
        login={login}
        hideContextChanges={hideContextChanges}
        onHideContextChanges={changeContextVisibility}
        progress={{
          reviewed: tour.flows.filter((f) => isFlowReviewed(f, ticked)).length,
          total: tour.flows.length,
        }}
        pendingCount={pendingCount}
        onOpenPending={() => go({ block: "pending_review" })}
        onHelp={() => setHelpOpen(true)}
      />
      <Workspace
        layout={layout}
        onChange={setLayout}
        mainRef={mainRef}
        nav={
          <NavList
            flows={tour.flows}
            quiet={tour.quiet}
            ticked={ticked}
            pendingCount={pendingCount}
            current={current}
            onSelect={go}
          />
        }
        main={centre}
        inspector={
          <InspectorPane
            streamLost={stream.lost}
            analysisId={analysisId}
            tour={tour}
            block={current}
            step={step}
            annotationId={annotationId}
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
            askRef={askRef}
            commentRef={commentRef}
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
      <TopBar login={login} />
      <main id="main" className="grid gap-3 px-8 py-5">
        {children}
      </main>
    </div>
  );
}
