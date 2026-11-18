import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
  type RefObject,
} from "react";
import type {
  Annotation,
  AnnotationId,
  AnnotationType,
  BlockRef,
  Evidence,
  GithubThread,
  PendingReview,
  Step,
  StepId,
  Tour,
} from "../../api/generated/types";
import { DifferingLinesBadge, ProvenanceBadge } from "../badge/Badge";
import { useFlashTarget } from "../flash/useFlashTarget";
import { GroupLabel, KindLabel, RiskLabel } from "../badge/LlmLabel";
import { FlowWorkLabel } from "../flow-intro/status";
import { annotationName, rangeText } from "../section/format";
import { EvidenceLinks } from "../section/EvidenceLinks";
import { Collapsible } from "./Collapsible";
import { useSectionState, type SectionControl } from "./sectionState";
import { Tooltip } from "../tooltip/Tooltip";
import {
  AssessmentSection,
  BehaviourSection,
  CallHierarchySection,
  ConsequencesSection,
  DiffSection,
  FlagsSection,
  ThreadsSection,
  WhySection,
} from "./StepSections";
import {
  FlowSummary,
  PendingSummary,
  PrContextSummary,
  QuietSummary,
  findStep,
  flowTitle,
  quietTitle,
} from "./summaries";
import { GithubLink } from "./text";

/** Props of the inspector panel. */
export interface InspectorProps {
  /** The analysis. */
  tour: Tour;
  /** The block shown in the centre; the inspector describes it. */
  block: BlockRef;
  /** The highlight the user clicked (an annotation of the current step); null = describe the block. */
  annotationId?: AnnotationId | null;
  /** Classic diff of the step or of the selected annotation's range (section 1), e.g. code-view `ClassicDiff`. */
  diff?: ReactNode;
  /** Dock under the sections: Ask, feedback, comment (e.g. `AskForm`, `FeedbackButtons`, `CommentComposer`). */
  dock?: ReactNode;
  /** The user's pending review, for the pending-review summary. */
  pendingReview?: PendingReview | null;
  /** Leaves the selected annotation ("Back to step"). */
  onBackToStep?: () => void;
  /** Opens a step (from the flow summary). */
  onSelectStep?: (step: StepId) => void;
  onEvidence?: (evidence: Evidence) => void;
  /** Scroll to and briefly highlight a remark or GitHub thread of the step ("Show in Inspector"); new `nonce`, again. */
  flash?: { kind: "annotation" | "thread"; id: string; nonce: number } | null;
  /** The `flash` request was handled (its highlight ended); it is not to be shown again. */
  onFlashed?: () => void;
  /** Section state shared with the dock (Ask); the panel keeps its own without it. */
  sections?: SectionControl;
  /** Dragged height in px of the dock (the Ask area); null = its natural height. */
  dockHeight?: number | null;
  /** A new dock height from the handle above it; null resets. Without it there is no handle. */
  onDockHeight?: (height: number | null) => void;
  /** "Use as review comment" on a flag of the step; no button without it. */
  onFlagAsComment?: (flag: Annotation) => void;
}

/** Smallest height in px of a resized dock: the Ask field and a few lines of the conversation. */
export const ASK_MIN_PX = 160;
/** Height change of one arrow key press on the dock's handle. */
export const DOCK_KEY_STEP_PX = 40;
// Room in px the Inspector keeps above a resized dock for its heading (its scroller's `min-h-30`).
const PANEL_KEEP_PX = 120;

/** The handle on the dock's top edge: drag, arrow keys, Home / End, Enter or double-click to reset. */
function DockHandle({
  inspector,
  dock,
  height,
  onHeight,
}: {
  inspector: RefObject<HTMLDivElement | null>;
  dock: RefObject<HTMLDivElement | null>;
  height: number | null;
  onHeight: (height: number | null) => void;
}) {
  const drag = useRef<{ y: number; from: number } | null>(null);
  // Drawn heights of the dock and the whole Inspector, for the separator's value and limits.
  const [drawn, setDrawn] = useState({ dock: 0, inspector: 0 });
  // After layout: the dock comes after this handle, so its ref is not set yet in a layout effect here.
  useEffect(() => {
    const measure = () =>
      setDrawn({
        dock: Math.round(dock.current?.getBoundingClientRect().height ?? 0),
        inspector: Math.round(inspector.current?.getBoundingClientRect().height ?? 0),
      });
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    for (const element of [dock.current, inspector.current]) if (element) observer.observe(element);
    return () => observer.disconnect();
  }, [dock, inspector]);
  const max = Math.max(ASK_MIN_PX, drawn.inspector - PANEL_KEEP_PX);
  const clamp = (px: number) => Math.round(Math.min(max, Math.max(ASK_MIN_PX, px)));
  // The drawn height (a stored one may no longer fit the window), within the limits.
  const current = () => clamp(drawn.dock || height || ASK_MIN_PX);
  const resize = (px: number | null) => onHeight(px === null ? null : clamp(px));
  const onKey = (event: KeyboardEvent<HTMLDivElement>) => {
    const next = {
      ArrowUp: current() + DOCK_KEY_STEP_PX,
      ArrowDown: current() - DOCK_KEY_STEP_PX,
      Home: ASK_MIN_PX,
      End: max,
    }[event.key];
    if (event.key === "Enter") resize(null);
    else if (next !== undefined) resize(next);
    else return;
    event.preventDefault();
  };
  return (
    <Tooltip
      tip="Drag to make Ask taller or shorter (the sections above make room); arrow keys too, Home and End for the limits; double-click or Enter resets."
      className="block"
    >
      {/* A 24 px hit area over the dock's top border; only a thin line shows. */}
      <div
        role="separator"
        aria-orientation="horizontal"
        aria-label="Resize the Ask area"
        tabIndex={0}
        aria-valuenow={drawn.dock || height ? current() : undefined}
        aria-valuemin={ASK_MIN_PX}
        aria-valuemax={max}
        onPointerDown={(event: PointerEvent<HTMLDivElement>) => {
          drag.current = { y: event.clientY, from: current() };
          event.currentTarget.setPointerCapture?.(event.pointerId);
          event.preventDefault();
        }}
        onPointerMove={(event: PointerEvent<HTMLDivElement>) => {
          if (drag.current) resize(drag.current.from + drag.current.y - event.clientY);
        }}
        onPointerUp={() => (drag.current = null)}
        onPointerCancel={() => (drag.current = null)}
        onDoubleClick={() => resize(null)}
        onKeyDown={onKey}
        className="relative z-10 -mb-3 -mt-3 h-6 shrink-0 cursor-row-resize touch-none after:absolute after:inset-x-0 after:top-1/2 after:h-0.5 after:-translate-y-1/2 after:bg-transparent hover:after:bg-accent focus-visible:outline-none focus-visible:after:bg-accent"
      />
    </Tooltip>
  );
}

// The section listing each annotation type.
const SECTION_OF: Record<AnnotationType, string> = {
  behaviour: "behaviour",
  note: "why",
  consequence: "consequences",
  flag: "flags",
};

const linesWord = (r: Annotation["range"]) =>
  `${r.start_line === r.end_line ? "line" : "lines"} ${rangeText(r)}`;

const overlaps = (t: GithubThread, step: Step, a: Annotation) =>
  t.path === step.facts.file &&
  t.side === step.facts.side &&
  t.line !== null &&
  (t.start_line ?? t.line) <= a.range.end_line &&
  t.line >= a.range.start_line;

interface Panel {
  title: string;
  /** Under the title: labels and links. */
  meta?: ReactNode;
  /** Where deterministic content comes from; LLM panels carry tags on their sections instead. */
  provenance?: string;
  body: ReactNode;
  /** Set when an annotation is selected: context line for Back to step. */
  backTo?: string;
}

function stepPanel(
  tour: Tour,
  step: Step,
  annotation: Annotation | undefined,
  diff: ReactNode,
  sections: SectionControl,
  onEvidence?: (evidence: Evidence) => void,
  onFlagAsComment?: (flag: Annotation) => void,
): Panel {
  const anns = annotation ? [annotation] : step.llm.annotations;
  const byType = (type: AnnotationType) => anns.filter((a) => a.type === type);
  const stepThreads = tour.threads.filter((t) => step.facts.thread_ids.includes(t.id));
  const stepTitle = `Step ${step.number} · ${step.llm.title}`;
  return {
    title: annotation ? `${annotationName(annotation)} · ${linesWord(annotation.range)}` : stepTitle,
    backTo: annotation ? stepTitle : undefined,
    meta: (
      <>
        <KindLabel kind={step.llm.kind} />
        <GithubLink
          url={step.facts.github_url}
          tip={`Open these lines on GitHub at the ${step.facts.side} commit (new tab)`}
        >
          {step.facts.file}:{rangeText(step.facts.range)}
        </GithubLink>
      </>
    ),
    body: (
      <>
        {step.facts.moved && (
          <Collapsible
            sectionKey="moved"
            title="Moved code"
            tone="facts"
            source="From the diff; no LLM."
            sections={sections}
          >
            <p>Moved from the base commit.</p>
            {step.facts.moved.badge ? (
              <ProvenanceBadge badge={step.facts.moved.badge} />
            ) : (
              <DifferingLinesBadge count={step.facts.moved.differing_lines} />
            )}
            <EvidenceLinks
              currentFile={step.facts.file}
              onEvidence={onEvidence}
              evidence={[
                ...new Set([step.facts.moved.from.range.start_line, step.facts.moved.from.range.end_line]),
              ].map((line) => ({ file: step.facts.moved!.from.file, side: "base", line }))}
            />
          </Collapsible>
        )}
        {[
          ["Called by", step.llm.called_by],
          ["Goes next", step.llm.next],
        ]
          .filter(([, text]) => text?.trim())
          .map(([title, text]) => (
            <Collapsible key={title} sectionKey={title!} title={title!} tone="llm" sections={sections}>
              <p>{text}</p>
            </Collapsible>
          ))}
        <DiffSection diff={diff} ofRange={annotation !== undefined} sections={sections} />
        <BehaviourSection
          currentFile={step.facts.file}
          onEvidence={onEvidence}
          annotations={annotation ? anns : byType("behaviour")}
          sections={sections}
        />
        <WhySection
          currentFile={step.facts.file}
          onEvidence={onEvidence}
          why={step.llm.why}
          notes={byType("note")}
          sections={sections}
        />
        <ConsequencesSection
          currentFile={step.facts.file}
          onEvidence={onEvidence}
          annotations={byType("consequence")}
          sections={sections}
        />
        <CallHierarchySection hierarchy={step.facts.call_hierarchy} sections={sections} />
        <FlagsSection
          currentFile={step.facts.file}
          step={step}
          onEvidence={onEvidence}
          annotations={byType("flag")}
          sections={sections}
          onUseAsComment={onFlagAsComment}
        />
        <AssessmentSection assessment={step.llm.assessment} sections={sections} />
        <ThreadsSection
          threads={annotation ? stepThreads.filter((t) => overlaps(t, step, annotation)) : stepThreads}
          status={tour.pr_context.facts.threads_status}
          sections={sections}
        />
      </>
    ),
  };
}

function panelFor(props: InspectorProps, sections: SectionControl): Panel {
  const { tour, block, annotationId, diff, pendingReview, onSelectStep } = props;
  const deterministic = "Deterministic: from the diff and the built-in rules; no LLM.";
  switch (block.block) {
    case "step": {
      const found = findStep(tour, block.step_id);
      if (!found) break;
      const annotation = found.step.llm.annotations.find((a) => a.id === annotationId);
      return stepPanel(tour, found.step, annotation, diff, sections, props.onEvidence, props.onFlagAsComment);
    }
    case "flow_intro": {
      const flow = tour.flows.find((f) => f.id === block.flow_id);
      if (!flow) break;
      return {
        title: flowTitle(flow),
        meta: (
          <>
            {flow.analysis_status && flow.analysis_status.state !== "analysed" ? (
              <FlowWorkLabel status={flow.analysis_status} analysis={tour.analysis.state} />
            ) : (
              <RiskLabel risk={flow.llm.risk} />
            )}
            <GroupLabel group={flow.group} />
          </>
        ),
        body: (
          <FlowSummary
            onEvidence={props.onEvidence}
            flow={flow}
            flows={tour.flows}
            onSelectStep={onSelectStep}
            sections={sections}
          />
        ),
      };
    }
    case "pr_context":
      return {
        title: "PR context",
        meta: (
          <span className="text-muted">
            #{tour.analysis.pr.number} {tour.analysis.pr.title}
          </span>
        ),
        body: (
          <PrContextSummary
            context={tour.pr_context}
            quiet={tour.quiet}
            flows={tour.flows}
            sections={sections}
          />
        ),
      };
    case "quiet":
      return {
        title: quietTitle(block.row),
        provenance: deterministic,
        body: (
          <QuietSummary row={block.row} quiet={tour.quiet} context={tour.pr_context} sections={sections} />
        ),
      };
    case "pending_review":
      return {
        title: "Pending review",
        provenance: "From your GitHub pending review; no LLM.",
        body: <PendingSummary review={pendingReview ?? null} sections={sections} />,
      };
  }
  return {
    title: "Inspector",
    provenance: deterministic,
    body: <p className="px-3 py-2">This block is not part of the analysis.</p>,
  };
}

/** Right-hand inspector (brief §4.3): never empty; the current block, or the selected annotation of a step. */
export function Inspector(props: InspectorProps) {
  const { dock, onBackToStep, flash, onFlashed, onDockHeight } = props;
  const own = useSectionState(JSON.stringify(props.block));
  const sections = props.sections ?? own;
  const panel = panelFor(props, sections);
  const heading = useRef<HTMLHeadingElement>(null);
  const focusHeading = useRef(false);
  const scroller = useRef<HTMLDivElement>(null);
  const root = useRef<HTMLDivElement>(null);
  const dockBox = useRef<HTMLDivElement>(null);
  // A collapsed Ask keeps the dock at its natural height (no empty room, no handle).
  const resizable = Boolean(dock && onDockHeight && sections.isOpen("ask"));
  const dockHeight = resizable ? (props.dockHeight ?? null) : null;
  const opened = useRef<number | null>(null);
  const attribute = flash?.kind === "thread" ? "data-thread-id" : "data-annotation-id";
  useFlashTarget(
    scroller,
    flash && { selector: `[${attribute}="${flash.id.replace(/["\\]/g, "\\$&")}"]`, nonce: flash.nonce },
    onFlashed,
  );

  // A flashed item in a collapsed section: open that section once per request.
  useEffect(() => {
    if (!flash || opened.current === flash.nonce) return;
    opened.current = flash.nonce;
    const step = props.block.block === "step" ? findStep(props.tour, props.block.step_id)?.step : undefined;
    const type = step?.llm.annotations.find((a) => a.id === flash.id)?.type;
    const key = flash.kind === "thread" ? "threads" : type && SECTION_OF[type];
    if (key && !sections.isOpen(key)) sections.reveal(key);
  });

  // After "Back to step" the button is gone; the heading takes the focus.
  useEffect(() => {
    if (focusHeading.current && !panel.backTo) {
      focusHeading.current = false;
      heading.current?.focus();
    }
  });

  return (
    <div ref={root} data-inspector className="flex h-full min-h-0 flex-col bg-surface">
      {/* A resized dock taller than the window allows shrinks; the panel keeps room for its heading. */}
      <div
        ref={scroller}
        tabIndex={0}
        className={`${dockHeight === null ? "min-h-0" : "min-h-30"} flex-1 overflow-auto overscroll-contain`}
      >
        <div className="px-3 pt-2.5 pb-1.5">
          {panel.backTo && onBackToStep && (
            <p className="mb-1 flex flex-wrap items-baseline gap-2 text-[12px]">
              <Tooltip tip="Shows the whole step again: all its LLM remarks, not only this one.">
                <button
                  type="button"
                  onClick={() => {
                    focusHeading.current = true;
                    onBackToStep();
                  }}
                  className="cursor-pointer text-accent underline"
                >
                  ← Back to step
                </button>
              </Tooltip>
              <span className="text-muted">{panel.backTo}</span>
            </p>
          )}
          <h2 ref={heading} tabIndex={-1} className="m-0 text-[15px] font-bold">
            {panel.title}
          </h2>
          {panel.meta && <p className="mt-1 flex flex-wrap items-center gap-2">{panel.meta}</p>}
          {panel.provenance && <p className="mt-1 text-[11.5px] text-muted">{panel.provenance}</p>}
          <p className="sr-only" aria-live="polite">
            {panel.title}
          </p>
        </div>
        {panel.body}
      </div>
      {resizable && onDockHeight && (
        <DockHandle inspector={root} dock={dockBox} height={dockHeight} onHeight={onDockHeight} />
      )}
      {dock && (
        // The Ask section (`data-dock-grow`) takes the room of a resized dock; the rest keep their height.
        <div
          ref={dockBox}
          role="group"
          aria-label="Ask, feedback and comment"
          data-dock
          data-sized={dockHeight === null ? undefined : true}
          style={dockHeight === null ? undefined : { height: `${dockHeight}px` }}
          className={`flex flex-col gap-2.5 overflow-auto border-t-2 border-border bg-surface-2 px-3 py-2.5 [&>*:not([data-dock-grow])]:shrink-0 ${dockHeight === null ? "max-h-[42vh] shrink-0" : "min-h-0 shrink"}`}
        >
          {dock}
        </div>
      )}
    </div>
  );
}
