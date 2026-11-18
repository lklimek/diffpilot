import { useEffect, useRef, type ReactNode } from "react";
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
import { GroupLabel, KindLabel, RiskLabel } from "../badge/LlmLabel";
import { FlowWorkLabel } from "../flow-intro/status";
import { rangeText } from "../section/format";
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
  QuestionsSection,
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
  /** Ask answers pinned to the current step (e.g. `AskAnswers`); shown for steps. */
  answers?: ReactNode;
  /** Dock under the sections: Ask, feedback, comment (e.g. `AskForm`, `FeedbackButtons`, `CommentComposer`). */
  dock?: ReactNode;
  /** The user's pending review, for the pending-review summary. */
  pendingReview?: PendingReview | null;
  /** Leaves the selected annotation ("Back to step"). */
  onBackToStep?: () => void;
  /** Opens a step (from the flow summary). */
  onSelectStep?: (step: StepId) => void;
  onEvidence?: (evidence: Evidence) => void;
}

const ANNOTATION: Record<AnnotationType, string> = {
  behaviour: "Behaviour",
  consequence: "Consequence",
  question: "Question to check",
  note: "Note",
};

const LLM_LINE = "LLM analysis unless marked otherwise.";

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
  /** "LLM analysis unless marked otherwise.", or where deterministic content comes from. */
  provenance: string;
  body: ReactNode;
  /** Set for a step: Ask answers apply. */
  step?: Step;
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
): Panel {
  const anns = annotation ? [annotation] : step.llm.annotations;
  const byType = (type: AnnotationType) => anns.filter((a) => a.type === type);
  const stepThreads = tour.threads.filter((t) => step.facts.thread_ids.includes(t.id));
  const stepTitle = `Step ${step.number} · ${step.llm.title}`;
  return {
    title: annotation ? `${ANNOTATION[annotation.type]} · ${linesWord(annotation.range)}` : stepTitle,
    backTo: annotation ? stepTitle : undefined,
    step,
    provenance: LLM_LINE,
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
        <QuestionsSection
          currentFile={step.facts.file}
          step={step}
          onEvidence={onEvidence}
          questions={byType("question")}
          sections={sections}
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
      return stepPanel(tour, found.step, annotation, diff, sections, props.onEvidence);
    }
    case "flow_intro": {
      const flow = tour.flows.find((f) => f.id === block.flow_id);
      if (!flow) break;
      return {
        title: flowTitle(flow),
        provenance: LLM_LINE,
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
        provenance: LLM_LINE,
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
  const { answers, dock, onBackToStep } = props;
  const sections = useSectionState();
  const panel = panelFor(props, sections);
  const heading = useRef<HTMLHeadingElement>(null);
  const focusHeading = useRef(false);

  // After "Back to step" the button is gone; the heading takes the focus.
  useEffect(() => {
    if (focusHeading.current && !panel.backTo) {
      focusHeading.current = false;
      heading.current?.focus();
    }
  });

  return (
    <div className="flex h-full min-h-0 flex-col bg-surface">
      <div tabIndex={0} className="min-h-0 flex-1 overflow-auto overscroll-contain">
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
          <p className={`mt-1 text-[11.5px] ${panel.provenance === LLM_LINE ? "text-llm" : "text-muted"}`}>
            {panel.provenance}
          </p>
          <p className="sr-only" aria-live="polite">
            {panel.title}
          </p>
        </div>
        {panel.body}
        {panel.step && answers && (
          <Collapsible sectionKey="ask" title="Ask: pinned answers" tone="llm" sections={sections}>
            {answers}
          </Collapsible>
        )}
      </div>
      {dock && (
        <div
          role="group"
          aria-label="Ask, feedback and comment"
          className="grid max-h-[42vh] gap-2.5 overflow-auto border-t-2 border-border bg-surface-2 px-3 py-2.5"
        >
          {dock}
        </div>
      )}
    </div>
  );
}
