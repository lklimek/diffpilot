import type { Flow } from "../../api/generated/types";
import { CodeText } from "../section/CodeText";
import { ProvenanceTag } from "../section/ProvenanceTag";
import { Tooltip } from "../tooltip/Tooltip";
import { FlowContextPin, type FlowContextPinProps } from "./FlowContextPin";

/** Whether an analysed flow has an introduction to show ("Why this flow"); pending flows have none. */
export const hasWhy = (flow: Flow) =>
  (flow.analysis_status?.state ?? "analysed") === "analysed" &&
  Boolean(flow.llm.introduction.problem_before.trim() || flow.llm.introduction.change.trim());

/** "Problem: … → Change: …" of the flow's introduction, with its LLM tag. */
export function FlowWhyText({ flow }: { flow: Flow }) {
  const intro = flow.llm.introduction;
  return (
    <>
      <span className="text-muted">Problem:</span> <CodeText text={intro.problem_before} />{" "}
      <span className="text-muted">→ Change:</span> <CodeText text={intro.change} />{" "}
      <ProvenanceTag source="llm" />
    </>
  );
}

/**
 * The collapsible "Why this flow" strip above a step's flow diagram (proposal §7): collapsed, one line with the
 * problem; open, problem → change and a link to the full introduction. Nothing for a flow without one.
 */
export function FlowWhy({
  flow,
  open,
  onToggle,
  onOpenIntro,
  pin,
}: {
  flow: Flow;
  open: boolean;
  onToggle: () => void;
  onOpenIntro: () => void;
  pin?: FlowContextPinProps;
}) {
  if (!hasWhy(flow)) return null;
  return (
    <section
      aria-label="Why this flow"
      className="mb-2 rounded-md border border-l-4 border-border border-l-llm bg-surface px-3 py-1 text-[12.5px]"
    >
      <div className="flex min-w-0 items-baseline gap-1.5">
        <Tooltip
          tip={
            open ? "Collapses this to one line; remembered." : "Shows the problem and the change; remembered."
          }
        >
          <button
            type="button"
            aria-expanded={open}
            onClick={onToggle}
            className="flex min-w-0 flex-1 cursor-pointer items-baseline gap-1.5 border-0 bg-transparent p-0 text-left text-text"
          >
            <span aria-hidden className="w-3 text-muted">
              {open ? "▾" : "▸"}
            </span>
            <b className="shrink-0 text-llm">Why this flow</b>
            {!open && (
              <span className="truncate text-muted">
                — <CodeText text={flow.llm.introduction.problem_before} linkPaths={false} />
              </span>
            )}
          </button>
        </Tooltip>
        {/* Collapsed, the LLM's one line keeps its tag outside the truncation. */}
        {!open && (
          <span className="shrink-0">
            <ProvenanceTag source="llm" />
          </span>
        )}
        {pin && <FlowContextPin section="Why this flow" {...pin} />}
      </div>
      {open && (
        <div className="grid gap-1 pt-1 pl-[18px]">
          <p className="m-0">
            <FlowWhyText flow={flow} />
          </p>
          <Tooltip tip="Opens the introduction of this flow: problem, change, outcome and its steps.">
            <button
              type="button"
              onClick={onOpenIntro}
              className="cursor-pointer justify-self-start border-0 bg-transparent p-0 text-accent underline"
            >
              Full introduction of flow {flow.position}
            </button>
          </Tooltip>
        </div>
      )}
    </section>
  );
}
