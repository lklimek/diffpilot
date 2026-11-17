import { Suspense, lazy, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Navigate, useParams } from "react-router";
import { ApiError } from "../../api/client";
import type {
  AnalysisId,
  AnnotationId,
  BlockRef,
  CommentTarget,
  LinesResponse,
  PendingComment,
  Step,
  StepId,
  Tour,
} from "../../api/generated/types";
import { useCommentActions, useLines, useMe, usePendingReview, useTour } from "../../api/queries";
import { useReviewStateStore } from "../../api/review-state";
import { BlockNav } from "../../components/block-nav/BlockNav";
import type { UnresolvedPosts } from "../../components/comment/CommentComposer";
import { FlowIntroBlock } from "../../components/flow-intro/FlowIntroBlock";
import { isFlowReviewed } from "../../components/flow-intro/progress";
import { HelpDialog } from "../../components/help/HelpDialog";
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
import { ChunkedCode, DiffCode, LinesCode, errorText, type CodeContext } from "./code";
import { InspectorPane, type CommentLineState } from "./InspectorPane";
import { loadLayout, saveLayout, showColumn, type TourLayout } from "./layout";
import { blockTitle, buildSequence, findStep, flowJump, sameBlock, stepQuery } from "./sequence";
import { Workspace } from "./Workspace";

// Loaded when the diagram is first shown (own chunk).
const FlowDiagram = lazy(() =>
  import("../../components/flow-diagram/FlowDiagram").then((m) => ({ default: m.FlowDiagram })),
);

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

function TourView({ analysisId, tour, review, login }: TourViewProps) {
  const { state, change: save, loadError, saveError } = review;
  const pending = usePendingReview(analysisId);
  const comments = useCommentActions(analysisId);
  const ticked = state.ticked_steps;

  const sequence = useMemo(() => buildSequence(tour), [tour]);
  const [current, setCurrent] = useState<BlockRef>(() => {
    const last = state.last_viewed;
    const known =
      last &&
      (sequence.some((b) => sameBlock(b, last)) || last.block === "quiet" || last.block === "pending_review");
    return known ? last : PR_CONTEXT;
  });
  const [annotationId, setAnnotationId] = useState<AnnotationId | null>(null);
  const [oldCode, setOldCode] = useState(false);
  const [commentLine, setCommentLine] = useState<CommentLineState | null>(null);
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
    setCurrent(block);
    setAnnotationId(null);
    setOldCode(false);
    setCommentLine(null);
    mainRef.current?.scrollTo?.({ top: 0 });
  };

  // The review screen never scrolls the page: no reserved scrollbar gutter, the columns get the full width.
  useEffect(() => {
    const style = document.documentElement.style;
    const before = style.scrollbarGutter;
    style.scrollbarGutter = "auto";
    return () => {
      style.scrollbarGutter = before;
    };
  }, []);

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
      ? pending.data.comments.length
      : ("unavailable" as const)
    : pending.isPending
      ? ("loading" as const)
      : undefined;
  const ctx: CodeContext = {
    analysisId,
    threads: tour.threads,
    comments: pendingComments,
    activeLine: commentLine?.target ?? null,
    onComment: comment,
  };
  const selectStep = (id: StepId) => go({ block: "step", step_id: id });

  let block: ReactNode;
  if (current.block === "flow_intro" && flow) {
    block = (
      <FlowIntroBlock
        flow={flow}
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
        code={
          oldCode ? (
            <DiffCode analysisId={analysisId} query={stepQuery(step)} />
          ) : (
            <LinesCode
              ctx={ctx}
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
      block = section && <NotAnalysedBlock section={section} counters={tour.pr_context.facts.counters} />;
    }
  } else if (current.block === "pending_review") {
    block = pending.isSuccess ? (
      <PendingReviewBlock
        review={pending.data}
        prUrl={tour.analysis.pr.html_url}
        onEdit={comments.edit}
        onDelete={comments.remove}
      />
    ) : pending.isError ? (
      <StatusMessage kind="error">Cannot read your pending review: {errorText(pending.error)}</StatusMessage>
    ) : (
      <StatusMessage kind="loading">Loading your pending review…</StatusMessage>
    );
  } else if (current.block === "pr_context") {
    block = <PrContextBlock pr={tour.analysis.pr} context={tour.pr_context} />;
  }

  const progressAlert =
    review.status === "unreadable"
      ? `Your ticks and last viewed block could not be read, so changes are not saved: ${errorText(loadError)}`
      : saveError !== null
        ? `Your ticks and last viewed block are not saved: ${errorText(saveError)}`
        : null;
  const centre = (
    <>
      {progressAlert && (
        <p
          role="alert"
          className="mb-2 rounded-md border border-l-4 border-high-bd bg-high-bg px-3 py-1.5 text-[12.5px]"
        >
          {progressAlert}
        </p>
      )}
      {flow && (
        <section aria-label="Flow diagram" className="mb-3 rounded-lg border border-border bg-surface">
          <button
            type="button"
            aria-expanded={layout.diagram}
            onClick={() => setLayout({ ...layout, diagram: !layout.diagram })}
            className="flex w-full cursor-pointer items-center gap-2 border-0 bg-transparent px-3 py-1.5 text-left text-[12.5px] font-semibold text-text"
          >
            <span aria-hidden>{layout.diagram ? "▾" : "▸"}</span>
            {layout.diagram ? "Hide flow diagram" : "Show flow diagram"}
            <span className="font-normal text-muted">
              Flow {flow.position}: {flow.llm.title}
            </span>
          </button>
          {layout.diagram && (
            <div className="max-h-[45vh] overflow-auto border-t border-border px-3 py-2">
              <Suspense fallback={<StatusMessage kind="loading">Loading the diagram…</StatusMessage>}>
                <FlowDiagram
                  graph={flow.graph}
                  title={flow.llm.title}
                  currentStepId={step?.id ?? null}
                  onSelectStep={selectStep}
                />
              </Suspense>
            </div>
          )}
        </section>
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
    <div className="flex h-screen flex-col">
      <a
        href="#main"
        className="absolute -left-[999px] z-[100] bg-surface px-2.5 py-1.5 focus:top-2 focus:left-2"
      >
        Skip to content
      </a>
      <TopBar
        pr={tour.analysis.pr}
        ci={tour.pr_context.facts.ci}
        login={login}
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
            analysisId={analysisId}
            tour={tour}
            block={current}
            step={step}
            annotationId={annotationId}
            onBackToStep={() => setAnnotationId(null)}
            onSelectStep={selectStep}
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
