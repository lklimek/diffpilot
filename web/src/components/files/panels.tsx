// The Files view's Inspector panels (brief §4.5): the file, a hunk, or the reader's selected lines.
import type { ReactNode } from "react";
import type {
  FileHunk,
  FlowId,
  GithubThread,
  LineSide,
  PrFile,
  StepId,
  Tour,
} from "../../api/generated/types";
import { githubLineUrl, type PrLinkBase } from "../github/links";
import { Collapsible } from "../inspector/Collapsible";
import type { InspectorPanel } from "../inspector/Inspector";
import type { SectionControl } from "../inspector/sectionState";
import { DiffSection, ThreadsSection } from "../inspector/StepSections";
import { GithubLink } from "../inspector/text";
import { rangeText } from "../section/format";
import { Tooltip } from "../tooltip/Tooltip";
import { fileDiffUrl } from "./FileHeader";
import { FlowsAtLines, NoFlowPart } from "./FlowsAtLines";
import { fileParts, flowsAt, hunkLabel, hunksIn, noFlowParts, partsIn, pathOn } from "./model";
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
  /** Classic diff of the hunk or selection. */
  diff?: ReactNode;
}

const BUTTON =
  "min-h-6 cursor-pointer rounded border border-border bg-surface px-2 text-[12.5px] text-text hover:border-accent";
const WHERE =
  "Where these lines are explained: from the analysis plan, which places every changed line exactly once. A step may also show lines another flow explains.";
const PER_STEP =
  "Behaviour, why, consequences, flags and assessment are per step: pick Details on a step above, or a ◆ in the code. A changed line without a ◆ has no LLM remark.";

const fileName = (path: string) => path.slice(path.lastIndexOf("/") + 1);

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

function FlowsSection({
  ctx,
  title,
  parts,
}: {
  ctx: FilesPanelContext;
  title: string;
  parts: FileHunk["parts"];
}) {
  const flows = flowsAt(ctx.tour, parts);
  const quiet = noFlowParts(parts);
  return (
    <Collapsible
      sectionKey="files-flows"
      title={`${title} (${flows.length})`}
      tone="facts"
      source={WHERE}
      sections={ctx.sections}
    >
      <div className="grid gap-2">
        {flows.length > 0 && (
          <FlowsAtLines
            flows={flows}
            ticked={ctx.ticked}
            analysis={ctx.tour.analysis.state}
            onDetails={ctx.onDetails}
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
            In no flow. Recognised by file path (rule <code>{file.group.rule}</code>), no LLM. The agent may
            read it as evidence; it does not explain it.
          </p>
        )}
        {flows.length > 0 && (
          <Collapsible
            sectionKey="files-flows"
            title={`Flows in this file (${flows.length})`}
            tone="facts"
            source={WHERE}
            sections={ctx.sections}
          >
            <FlowsAtLines
              flows={flows}
              ticked={ctx.ticked}
              analysis={tour.analysis.state}
              onDetails={ctx.onDetails}
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
        <DiffSection diff={ctx.diff} ofRange sections={ctx.sections} />
        <ThreadsSection
          threads={threads}
          status={ctx.tour.pr_context.facts.threads_status}
          sections={ctx.sections}
        />
        <p className="px-3 py-1.5 text-[12px] text-muted">{PER_STEP}</p>
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
