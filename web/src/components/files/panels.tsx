// The Files view's Inspector panels (brief §4.5): the file, a hunk, or the reader's selected lines.
import type { ReactNode } from "react";
import type {
  Annotation,
  Evidence,
  FileHunk,
  FlowId,
  GithubThread,
  LineRange,
  LineSide,
  PrFile,
  StepId,
  Tour,
} from "../../api/generated/types";
import { ProvenanceBadge } from "../badge/Badge";
import { KindLabel } from "../badge/LlmLabel";
import { FlagCard } from "../flag/FlagCard";
import { sortedFlags } from "../flag/flags";
import { fileDiffUrl, githubLineUrl, type PrLinkBase } from "../github/links";
import { Collapsible } from "../inspector/Collapsible";
import { StepDetails, type InspectorPanel } from "../inspector/Inspector";
import type { SectionControl } from "../inspector/sectionState";
import { DiffSection, ThreadsSection } from "../inspector/StepSections";
import { GithubLink } from "../inspector/text";
import { rangeText } from "../section/format";
import { Tooltip } from "../tooltip/Tooltip";
import { FlowsAtLines, FlowsInFile, NoFlowPart } from "./FlowsAtLines";
import {
  fileName,
  fileParts,
  flowsAt,
  hunkLabel,
  hunksIn,
  noFlowParts,
  partsIn,
  pathOn,
  type StepAtLines,
} from "./model";
import { VIEWED_LOOK } from "./ViewedChip";

/** What every Files panel is built from, and its callbacks. */
export interface FilesPanelContext {
  tour: Tour;
  file: PrFile;
  pr: PrLinkBase;
  /** Steps ticked in the Flows view (read-only). */
  ticked: readonly StepId[];
  sections: SectionControl;
  onSelectHunk: (hunk: string) => void;
  onDetails: (step: StepId) => void;
  onOpenInFlows: (step: StepId) => void;
  onRequestFlow?: (flow: FlowId) => void;
  requestBusy?: boolean;
  /** Classic diff of the hunk, the narrowed step's lines or the selection. */
  diff?: ReactNode;
  onEvidence?: (evidence: Evidence) => void;
  onFlagAsComment?: (flag: Annotation) => void;
}

const BUTTON =
  "min-h-6 cursor-pointer rounded border border-border bg-surface px-2 text-[12.5px] text-text hover:border-accent";
const WHERE =
  "Where these lines are explained: from the analysis plan, which places every changed line exactly once. A step may also show lines another flow explains.";
const PER_STEP =
  "Behaviour, why, consequences, flags and assessment are per step: pick Details on a step above, or a ◆ in the code. A changed line without a ◆ has no LLM remark.";

/** A deterministic label of what the reader is looking at ("hunk", "your selection"). */
function Kind({ text, tip }: { text: string; tip: string }) {
  return (
    <Tooltip tip={tip}>
      <span
        tabIndex={0}
        className="cursor-help rounded border border-mv-bd bg-surface-2 px-1.5 text-[11px] leading-[18px] font-semibold"
      >
        {text}
      </span>
    </Tooltip>
  );
}

const threadsAt = (tour: Tour, file: PrFile, side: LineSide, from: number, to: number) =>
  tour.threads.filter(
    (t) =>
      t.path === pathOn(file, side) &&
      t.side === side &&
      t.line !== null &&
      (t.start_line ?? t.line) <= to &&
      t.line >= from,
  );

/** One-line owners of a hunk: "flow 1 · 7 steps", "Unexplained", "moved, text identical"… */
function hunkOwners(ctx: FilesPanelContext, hunk: FileHunk): string {
  const flows = flowsAt(ctx.tour, hunk.parts).map((f) =>
    f.steps.length > 0
      ? `flow ${f.flow.position} · ${f.steps.length} ${f.steps.length === 1 ? "step" : "steps"}`
      : `flow ${f.flow.position} · not analysed yet`,
  );
  const quiet = [
    ...new Set(
      noFlowParts(hunk.parts).map(
        (p) =>
          ({
            unexplained: "Unexplained",
            not_analysed: "not analysed (path rule)",
            context_rule: "line rule",
            identical_move: "moved, text identical",
            whitespace_only: "whitespace only",
            step: "",
            flow: "",
          })[p.owner.kind],
      ),
    ),
  ];
  return [...flows, ...quiet].join("; ");
}

/** Steps listed at some parts, over all their flows. */
const stepCount = (flows: ReturnType<typeof flowsAt>) => flows.reduce((n, f) => n + f.steps.length, 0);

function FlowsSection({
  ctx,
  title,
  parts,
  only,
}: {
  ctx: FilesPanelContext;
  title: string;
  parts: FileHunk["parts"];
  /** The narrowed step: only its row, no other flow and no parts in no flow. */
  only?: StepId;
}) {
  const all = flowsAt(ctx.tour, parts);
  const flows = only
    ? all.flatMap((f) => {
        const steps = f.steps.filter((s) => s.step.id === only);
        return steps.length > 0 ? [{ ...f, planned: false, steps }] : [];
      })
    : all;
  const quiet = only ? [] : noFlowParts(parts);
  return (
    <Collapsible
      sectionKey="files-flows"
      title={flows.length > 0 ? `${title} (${flows.length})` : title}
      tone="facts"
      source={flows.length > 0 ? WHERE : undefined}
      sections={ctx.sections}
    >
      <div className="grid gap-2">
        {flows.length > 0 && (
          <FlowsAtLines
            flows={flows}
            ticked={ctx.ticked}
            analysis={ctx.tour.analysis.state}
            onDetails={stepCount(flows) > 1 ? ctx.onDetails : undefined}
            onOpenInFlows={ctx.onOpenInFlows}
            onRequestFlow={ctx.onRequestFlow}
            requestBusy={ctx.requestBusy}
          />
        )}
        {quiet.map((part) => (
          <NoFlowPart key={`${part.side}:${part.range.start_line}`} part={part} />
        ))}
      </div>
    </Collapsible>
  );
}

function HunkList({ ctx, hunks }: { ctx: FilesPanelContext; hunks: readonly FileHunk[] }) {
  return (
    <ul className="m-0 grid list-none gap-1 p-0">
      {hunks.map((hunk) => {
        const remarks = flowsAt(ctx.tour, hunk.parts)
          .flatMap((f) => f.steps)
          .filter((s) => s.role === "explained")
          .reduce((n, s) => n + s.step.llm.annotations.length, 0);
        return (
          <li key={hunk.id} className="flex flex-wrap items-baseline gap-x-2">
            <Tooltip tip="Selects this hunk: the Inspector lists every flow and step in it.">
              <button
                type="button"
                onClick={() => ctx.onSelectHunk(hunk.id)}
                className="min-h-6 cursor-pointer text-left text-accent underline"
              >
                {hunkLabel(hunk)}
              </button>
            </Tooltip>
            <span className="text-[12px] text-muted">{hunkOwners(ctx, hunk)}</span>
            {remarks > 0 && (
              <span className="text-llm">
                <span aria-hidden>{"◆".repeat(Math.min(remarks, 8))}</span>
                <span className="sr-only">{`${remarks} LLM ${remarks === 1 ? "remark" : "remarks"}`}</span>
              </span>
            )}
          </li>
        );
      })}
    </ul>
  );
}

/** The file summary: flows in the file with their ticks, its hunks, its GitHub threads. Never empty. */
export function filePanel(ctx: FilesPanelContext): InspectorPanel {
  const { file, tour } = ctx;
  const look = VIEWED_LOOK[file.viewed ?? "unread"];
  const flows = flowsAt(tour, fileParts(file));
  const threads = tour.threads.filter((t) => t.path === file.path || t.path === file.old_path);
  return {
    title: file.path,
    meta: (
      <>
        <span className="font-mono text-[12px]">
          <span className="text-add">+{file.added}</span> <span className="text-high">−{file.removed}</span>
        </span>
        <span className="text-[12px]">
          <span aria-hidden className={look.className}>
            {look.glyph}{" "}
          </span>
          {file.viewed === "viewed"
            ? "viewed on GitHub"
            : file.viewed === null
              ? look.text
              : `${look.text} on GitHub`}
        </span>
      </>
    ),
    body: (
      <>
        {file.not_rendered && (
          <p className="px-3 py-1 text-[12.5px]">
            Not shown:{" "}
            {file.not_rendered === "binary"
              ? "a binary file"
              : file.not_rendered === "generated"
                ? "a generated file"
                : "larger than the code reader's limit"}
            . Its code is never rendered; the counters of the PR context count it.
          </p>
        )}
        {file.group.kind === "not_analysed" && flows.length === 0 && (
          <p className="px-3 py-1 text-[12.5px]">
            In no flow. Recognised by file path (rule <code>{file.group.rule}</code>){" "}
            <ProvenanceBadge badge={file.group.badge} />, no LLM. The agent may read it as evidence; it does
            not explain it.
          </p>
        )}
        {flows.length > 0 && (
          <Collapsible
            sectionKey="files-flows"
            title={`Flows in this file (${flows.length})`}
            tone="facts"
            sections={ctx.sections}
          >
            <FlowsInFile
              flows={flows}
              ticked={ctx.ticked}
              analysis={tour.analysis.state}
              onOpenInFlows={ctx.onOpenInFlows}
              onRequestFlow={ctx.onRequestFlow}
              requestBusy={ctx.requestBusy}
            />
          </Collapsible>
        )}
        {file.hunks.length > 0 && (
          <Collapsible
            sectionKey="files-hunks"
            title={`Hunks in this file (${file.hunks.length})`}
            tone="facts"
            source="From the diff; flows from the analysis plan."
            sections={ctx.sections}
          >
            <HunkList ctx={ctx} hunks={file.hunks} />
          </Collapsible>
        )}
        <ThreadsSection
          threads={threads}
          status={tour.pr_context.facts.threads_status}
          sections={ctx.sections}
        />
      </>
    ),
  };
}

/** A hunk: every flow and step in it, the parts in no flow, its classic diff. */
export function hunkPanel(ctx: FilesPanelContext, hunk: FileHunk): InspectorPanel {
  const side: LineSide = hunk.head ? "head" : "base";
  const range = (hunk.head ?? hunk.base)!;
  const url = githubLineUrl(
    ctx.pr,
    { path: pathOn(ctx.file, side), side, start: range.start_line, end: range.end_line },
    true,
  );
  const threads: GithubThread[] = [
    ...(hunk.head ? threadsAt(ctx.tour, ctx.file, "head", hunk.head.start_line, hunk.head.end_line) : []),
    ...(hunk.base ? threadsAt(ctx.tour, ctx.file, "base", hunk.base.start_line, hunk.base.end_line) : []),
  ];
  const flows = flowsAt(ctx.tour, hunk.parts);
  const steps = flows.flatMap((f) => f.steps);
  const single = steps.length === 1 && steps[0]!.role === "explained" ? steps[0]! : undefined;
  return {
    title: `${fileName(ctx.file.path)} · hunk, ${hunkLabel(hunk)}`,
    meta: (
      <>
        <Kind
          text="hunk"
          tip="A run of changed lines of the diff, removed and added together, without context (deterministic)."
        />
        <GithubLink url={url} tip="Open these changed lines in the pull request diff on GitHub (new tab).">
          {ctx.file.path}:{rangeText(range)}
        </GithubLink>
      </>
    ),
    body: (
      <>
        <FlowsSection ctx={ctx} title="Flows" parts={hunk.parts} />
        <DiffSection diff={ctx.diff} ofRange title="Classic diff of this hunk" sections={ctx.sections} />
        {single ? (
          <StepDetails
            files
            tour={ctx.tour}
            step={single.step}
            sections={ctx.sections}
            onEvidence={ctx.onEvidence}
            onFlagAsComment={ctx.onFlagAsComment}
          />
        ) : (
          <HunkFlags ctx={ctx} hunk={hunk} steps={steps} />
        )}
        <ThreadsSection
          threads={threads}
          status={ctx.tour.pr_context.facts.threads_status}
          sections={ctx.sections}
        />
        {steps.length > 1 && <p className="px-3 py-1.5 text-[12px] text-muted">{PER_STEP}</p>}
      </>
    ),
  };
}

/** The flags of every step explained in a hunk, strongest first, each naming its step. */
function HunkFlags({
  ctx,
  hunk,
  steps,
}: {
  ctx: FilesPanelContext;
  hunk: FileHunk;
  steps: readonly StepAtLines[];
}) {
  const flags = steps
    .filter((s) => s.role === "explained")
    .flatMap(({ flow, step }) => {
      const range = step.facts.side === "head" ? hunk.head : hunk.base;
      const here = step.llm.annotations.filter(
        (a) =>
          a.type === "flag" &&
          a.text?.trim() &&
          range &&
          a.range.start_line <= range.end_line &&
          a.range.end_line >= range.start_line,
      );
      return sortedFlags(here).map((a) => ({
        a,
        step,
        owner: `step ${step.number} of flow ${flow.position}`,
      }));
    });
  if (flags.length === 0) return null;
  return (
    <Collapsible sectionKey="flags" title={`Flags (${flags.length})`} tone="llm" sections={ctx.sections}>
      <ul className="m-0 grid list-none gap-2 p-0">
        {flags.map(({ a, step, owner }) => (
          <FlagCard
            key={a.id}
            annotation={a}
            owner={owner}
            currentFile={step.facts.file}
            onEvidence={ctx.onEvidence}
            onOpen={
              ctx.onEvidence &&
              (() =>
                ctx.onEvidence!({ file: step.facts.file, side: step.facts.side, line: a.range.start_line }))
            }
            onUseAsComment={ctx.onFlagAsComment && (() => ctx.onFlagAsComment!(a))}
          />
        ))}
      </ul>
    </Collapsible>
  );
}

const rangesWord = (ranges: readonly LineRange[], prefix: string) =>
  ranges.length === 0
    ? []
    : [
        `${prefix}${ranges.length === 1 && ranges[0]!.start_line === ranges[0]!.end_line ? "line" : "lines"} ${ranges.map(rangeText).join(", ")}`,
      ];

/** A step narrowed from a hunk: its flow row and tick, the lines it explains here, its sections. */
export function stepInHunkPanel(
  ctx: FilesPanelContext,
  hunk: FileHunk,
  id: StepId,
): InspectorPanel | undefined {
  const at = flowsAt(ctx.tour, hunk.parts)
    .flatMap((f) => f.steps)
    .find((s) => s.step.id === id);
  if (!at) return undefined;
  const { step, flow } = at;
  const where = [...rangesWord(at.lines, ""), ...rangesWord(at.removed, "removed ")].join(" · ");
  const first = at.lines[0] ?? at.removed[0]!;
  const side: LineSide = at.lines.length > 0 ? "head" : "base";
  const last = (at.lines.length > 0 ? at.lines : at.removed).at(-1)!;
  const url = githubLineUrl(
    ctx.pr,
    { path: pathOn(ctx.file, side), side, start: first.start_line, end: last.end_line },
    true,
  );
  const threads = threadsAt(ctx.tour, ctx.file, side, first.start_line, last.end_line);
  return {
    title: `Step ${step.number} of flow ${flow.position} · ${step.llm.title}`,
    meta: (
      <>
        <KindLabel kind={step.llm.kind} />
        <GithubLink url={url} tip="Open these changed lines in the pull request diff on GitHub (new tab).">
          {pathOn(ctx.file, side)}:{rangeText({ start_line: first.start_line, end_line: last.end_line })}
        </GithubLink>
        <span className="basis-full text-[12px] text-muted">
          {at.role === "explained"
            ? `Explains in this hunk: ${where}`
            : `Shows these lines of another flow as context: ${where}`}
        </span>
      </>
    ),
    body: (
      <>
        <FlowsSection ctx={ctx} title="Flows" parts={hunk.parts} only={id} />
        <DiffSection diff={ctx.diff} ofRange title="Classic diff of these lines" sections={ctx.sections} />
        <StepDetails
          files
          tour={ctx.tour}
          step={step}
          sections={ctx.sections}
          onEvidence={ctx.onEvidence}
          onFlagAsComment={ctx.onFlagAsComment}
        />
        <ThreadsSection
          threads={threads}
          status={ctx.tour.pr_context.facts.threads_status}
          sections={ctx.sections}
        />
      </>
    ),
  };
}

/** The reader's own block: the selected lines, what they touch, their classic diff, Ask and Comment. */
export function linesPanel(
  ctx: FilesPanelContext,
  selection: { side: LineSide; from: number; to: number },
  actions: { onAsk: () => void; onComment?: () => void },
): InspectorPanel {
  const { file } = ctx;
  const { side, from, to } = selection;
  const parts = partsIn(file, side, from, to);
  const hunks = hunksIn(file, side, from, to);
  const range = { start_line: from, end_line: to };
  const url =
    parts.length > 0
      ? githubLineUrl(ctx.pr, { path: pathOn(file, side), side, start: from, end: to }, true)
      : fileDiffUrl(ctx.pr, file.path);
  const lines = `${from === to ? "line" : "lines"} ${rangeText(range)}${side === "base" ? " (removed)" : ""}`;
  return {
    title: `Your selection · ${fileName(file.path)} ${lines}`,
    meta: (
      <>
        <Kind text="your selection" tip="Lines you selected yourself; they are their own block here." />
        <GithubLink url={url} tip="Open these lines on GitHub (new tab).">
          {file.path}:{rangeText(range)}
        </GithubLink>
      </>
    ),
    body: (
      <>
        <div className="flex flex-wrap gap-2 px-3 py-1.5">
          <Tooltip tip="Asks the LLM about exactly these lines (a); the step is named only when they lie in one step.">
            <button type="button" onClick={actions.onAsk} className={BUTTON}>
              Ask about these lines
            </button>
          </Tooltip>
          {actions.onComment && (
            <Tooltip tip={`Opens the comment composer on line ${to} (c), the last line of the selection.`}>
              <button type="button" onClick={actions.onComment} className={BUTTON}>
                Comment on line {to}
              </button>
            </Tooltip>
          )}
        </div>
        {parts.length === 0 ? (
          <p className="px-3 py-1 text-[12.5px]">
            Unchanged code: no line of this selection is changed by this PR.
          </p>
        ) : (
          <>
            {hunks.length > 0 && (
              <Collapsible
                sectionKey="files-hunks"
                title={`Hunks in this selection (${hunks.length})`}
                tone="facts"
                source="From the diff."
                sections={ctx.sections}
              >
                <HunkList ctx={ctx} hunks={hunks} />
              </Collapsible>
            )}
            <FlowsSection ctx={ctx} title="In this selection: flows" parts={parts} />
          </>
        )}
        <DiffSection diff={ctx.diff} ofRange sections={ctx.sections} />
      </>
    ),
  };
}
