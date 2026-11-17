import { useId, type ReactNode } from "react";
import type { BlockRef, Flow, QuietRowRef, QuietRows, StepId } from "../../api/generated/types";
import { GroupLabel, RiskLabel } from "../badge/LlmLabel";
import { isFlowReviewed } from "../flow-intro/progress";
import { NOT_ANALYSED_LABEL, fileCount, lineCount, plural } from "../quiet/labels";
import { Tooltip } from "../tooltip/Tooltip";

interface NavListProps {
  /** Flows in tour order. */
  flows: Flow[];
  /** Quiet rows of the analysis, listed after every flow. */
  quiet: QuietRows;
  /** Steps the user ticked. */
  ticked: StepId[];
  /** Comments in the pending review; `"unavailable"` without the GitHub App; no row when omitted. */
  pendingCount?: number | `${number}+` | "unavailable" | "loading";
  /** The block shown in the centre. */
  current: BlockRef;
  /** Called with the block the user picks. */
  onSelect: (block: BlockRef) => void;
}

const ITEM =
  "block w-full cursor-pointer rounded-md border-l-4 border-transparent px-2 py-1.5 text-left text-text hover:bg-surface-2 aria-[current]:border-accent aria-[current]:bg-accent-bg aria-[current]:font-semibold";
const QUIET_ITEM = `${ITEM} text-[12.5px] text-mv`;
const SMALL = "block text-[11px] font-normal text-muted";
const TICK = (
  <span aria-hidden className="float-right font-bold text-accent">
    ☑
  </span>
);

const currentFlowId = (flows: Flow[], current: BlockRef): string | undefined => {
  if (current.block === "flow_intro") return current.flow_id;
  if (current.block === "step") return flows.find((f) => f.steps.some((s) => s.id === current.step_id))?.id;
  return undefined;
};

const ariaCurrent = (isCurrent: boolean) => (isCurrent ? ("step" as const) : undefined);

const sameRow = (current: BlockRef, row: QuietRowRef) =>
  current.block === "quiet" && JSON.stringify(current.row) === JSON.stringify(row);

function Heading({ children }: { children: ReactNode }) {
  return <h2 className="mt-3 mb-1 text-[10.5px] font-bold tracking-wide text-muted uppercase">{children}</h2>;
}

interface QuietRowProps {
  row: QuietRowRef;
  tip: string;
  current: BlockRef;
  onSelect: (block: BlockRef) => void;
  children: ReactNode;
}

function QuietRow({ row, tip, current, onSelect, children }: QuietRowProps) {
  return (
    <li>
      <Tooltip tip={tip} className="block" placement="right">
        <button
          type="button"
          className={QUIET_ITEM}
          aria-current={sameRow(current, row) || undefined}
          onClick={() => onSelect({ block: "quiet", row })}
        >
          {children}
        </button>
      </Tooltip>
    </li>
  );
}

function pendingText(count: NonNullable<NavListProps["pendingCount"]>): string {
  if (count === "unavailable") return "not available";
  if (count === "loading") return "loading…";
  // "N+": GitHub has more pending comments than were read.
  return typeof count === "string" ? `${count} comments` : plural(count, "comment");
}

/** Left menu: PR context, flows in tour order with risk and ticks, quiet rows, pending review. */
export function NavList({ flows, quiet, ticked, pendingCount, current, onSelect }: NavListProps) {
  const openFlow = currentFlowId(flows, current);
  const unexplainedLines = quiet.unexplained.reduce((n, f) => n + lineCount(f.ranges), 0);
  const sections = quiet.not_analysed.filter((s) => s.items.length > 0);
  const rowProps = { current, onSelect };
  const contextHint = useId();
  return (
    <div className="text-sm">
      <button
        type="button"
        data-row="menu"
        className={`${ITEM} mb-1 font-semibold`}
        aria-current={ariaCurrent(current.block === "pr_context")}
        aria-label="PR context"
        aria-describedby={contextHint}
        onClick={() => onSelect({ block: "pr_context" })}
      >
        PR context
        <span id={contextHint} className={SMALL}>
          Problem, related work, fit questions, CI
        </span>
      </button>
      <Heading>Flows · most important first</Heading>
      <ol className="m-0 list-none p-0">
        {flows.map((flow) => {
          const reviewed = isFlowReviewed(flow, ticked);
          return (
            <li key={flow.id} className="mb-1">
              <div className="flex items-start gap-1">
                <button
                  type="button"
                  data-row="menu"
                  className={`${ITEM} flex-1`}
                  aria-current={ariaCurrent(current.block === "flow_intro" && current.flow_id === flow.id)}
                  onClick={() => onSelect({ block: "flow_intro", flow_id: flow.id })}
                  aria-label={`Flow ${flow.position}: ${flow.llm.title}${reviewed ? ", reviewed" : ""}`}
                >
                  {reviewed && TICK}
                  <b>{flow.position}</b> {flow.llm.title}
                </button>
                <span className="flex flex-col items-end gap-0.5 pt-1.5">
                  {flow.analysis_status && flow.analysis_status.state !== "analysed" ? (
                    <span className="text-xs">
                      {flow.analysis_status.state === "analysing"
                        ? "Being analysed"
                        : flow.analysis_status.state === "failed"
                          ? "Failed"
                          : flow.analysis_status.state === "queued"
                            ? "Queued"
                            : "Not analysed yet"}
                    </span>
                  ) : (
                    <RiskLabel risk={flow.llm.risk} />
                  )}
                  <GroupLabel group={flow.group} />
                </span>
              </div>
              {flow.id === openFlow && (
                <ol className="my-0.5 mb-1.5 ml-3.5 list-none border-l-2 border-border py-0 pl-1.5">
                  {flow.steps.map((step) => {
                    const done = ticked.includes(step.id);
                    return (
                      <li key={step.id}>
                        <button
                          type="button"
                          className={`${ITEM} py-0.5 text-[12.5px]`}
                          aria-current={ariaCurrent(current.block === "step" && current.step_id === step.id)}
                          onClick={() => onSelect({ block: "step", step_id: step.id })}
                          aria-label={`Step ${step.number}: ${step.llm.title}${done ? ", reviewed" : ""}`}
                        >
                          <span aria-hidden className="inline-block w-3.5 font-bold text-accent">
                            {done ? "☑" : ""}
                          </span>
                          <b>{step.number}</b> {step.llm.title}
                        </button>
                      </li>
                    );
                  })}
                </ol>
              )}
            </li>
          );
        })}
      </ol>
      {(quiet.unexplained.length > 0 || quiet.moves.length > 0) && (
        <>
          <Heading>Quiet rows</Heading>
          <ul aria-label="Quiet rows" className="m-0 list-none p-0">
            {quiet.unexplained.length > 0 && (
              <QuietRow
                row={{ row: "unexplained" }}
                tip="Changed lines the analysis did not assign to any flow. Read them yourself."
                {...rowProps}
              >
                <strong>Unexplained — read these yourself</strong>
                <small className={SMALL}>
                  {plural(unexplainedLines, "changed line")} in {plural(fileCount(quiet.unexplained), "file")}
                </small>
              </QuietRow>
            )}
            {quiet.moves.length > 0 && (
              <QuietRow
                row={{ row: "moves" }}
                tip="Moved blocks whose text is identical to the original after whitespace normalisation (deterministic check)."
                {...rowProps}
              >
                Moves with identical text
                <small className={SMALL}>{plural(quiet.moves.length, "moved block")}</small>
              </QuietRow>
            )}
          </ul>
        </>
      )}
      {sections.length > 0 && (
        <>
          <Heading>Not analysed</Heading>
          <ul aria-label="Not analysed" className="m-0 list-none p-0">
            {sections.map((s) => (
              <QuietRow
                key={s.category}
                row={{ row: "not_analysed", category: s.category }}
                tip="Recognised by a built-in path or line rule; the LLM did not analyse it."
                {...rowProps}
              >
                {NOT_ANALYSED_LABEL[s.category]} · {plural(s.file_count, "file")}
              </QuietRow>
            ))}
          </ul>
        </>
      )}
      {pendingCount !== undefined && (
        <>
          <Heading>Pending review</Heading>
          <Tooltip
            tip="Your pending review on GitHub: the comments you added, not yet submitted."
            className="block"
            placement="right"
          >
            <button
              type="button"
              data-row="menu"
              className={ITEM}
              aria-current={current.block === "pending_review" || undefined}
              aria-label={`Pending review on GitHub: ${pendingText(pendingCount)}`}
              onClick={() => onSelect({ block: "pending_review" })}
            >
              Pending review on GitHub{" "}
              <b className="ml-1 rounded-full bg-surface-2 px-1.5 text-xs whitespace-nowrap">
                {pendingText(pendingCount)}
              </b>
            </button>
          </Tooltip>
        </>
      )}
      <p className="mt-3 text-xs text-muted">
        <Kbd>j</Kbd> <Kbd>k</Kbd> next / previous block · <Kbd>n</Kbd> <Kbd>p</Kbd> flow · <Kbd>?</Kbd> help
      </p>
    </div>
  );
}

function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="rounded border border-b-2 border-border px-1">{children}</kbd>;
}
