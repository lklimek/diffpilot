import type { BlockRef, Flow } from "../../api/generated/types";
import { GroupLabel, RiskLabel } from "../badge/LlmLabel";

interface NavListProps {
  /** Flows in tour order. */
  flows: Flow[];
  /** The block shown in the centre. */
  current: BlockRef;
  /** Called with the block the user picks. */
  onSelect: (block: BlockRef) => void;
}

const ITEM =
  "block w-full cursor-pointer rounded-md border-l-4 border-transparent px-2 py-1.5 text-left text-text hover:bg-surface-2 aria-[current]:border-accent aria-[current]:bg-accent-bg aria-[current]:font-semibold";

const currentFlowId = (flows: Flow[], current: BlockRef): string | undefined => {
  if (current.block === "flow_intro") return current.flow_id;
  if (current.block === "step") return flows.find((f) => f.steps.some((s) => s.id === current.step_id))?.id;
  return undefined;
};

const ariaCurrent = (isCurrent: boolean) => (isCurrent ? ("step" as const) : undefined);

/** Left menu: PR context, flows in tour order with their LLM risk, the current flow's numbered steps. */
export function NavList({ flows, current, onSelect }: NavListProps) {
  const openFlow = currentFlowId(flows, current);
  return (
    <div className="text-sm">
      <button
        type="button"
        className={`${ITEM} mb-3 text-base font-semibold`}
        aria-current={ariaCurrent(current.block === "pr_context")}
        onClick={() => onSelect({ block: "pr_context" })}
      >
        PR context
      </button>
      <h2 className="mb-1 text-[10.5px] font-bold tracking-wide text-muted uppercase">
        Flows · most important first
      </h2>
      <ol className="m-0 list-none p-0">
        {flows.map((flow) => (
          <li key={flow.id} className="mb-1">
            <div className="flex items-start gap-1">
              <button
                type="button"
                className={`${ITEM} flex-1`}
                aria-current={ariaCurrent(current.block === "flow_intro" && current.flow_id === flow.id)}
                onClick={() => onSelect({ block: "flow_intro", flow_id: flow.id })}
                aria-label={`Flow ${flow.position}: ${flow.llm.title}`}
              >
                <b>{flow.position}</b> {flow.llm.title}
              </button>
              <span className="flex flex-col items-end gap-0.5 pt-1.5">
                <RiskLabel risk={flow.llm.risk} />
                <GroupLabel group={flow.group} />
              </span>
            </div>
            {flow.id === openFlow && (
              <ol className="my-0.5 mb-1.5 ml-3.5 list-none border-l-2 border-border py-0 pl-1.5">
                {flow.steps.map((step) => (
                  <li key={step.id}>
                    <button
                      type="button"
                      className={`${ITEM} py-0.5 text-[12.5px]`}
                      aria-current={ariaCurrent(current.block === "step" && current.step_id === step.id)}
                      onClick={() => onSelect({ block: "step", step_id: step.id })}
                      aria-label={`Step ${step.number}: ${step.llm.title}`}
                    >
                      <b>{step.number}</b> {step.llm.title}
                    </button>
                  </li>
                ))}
              </ol>
            )}
          </li>
        ))}
      </ol>
      <p className="mt-3 text-xs text-muted">
        <kbd className="rounded border border-b-2 border-border px-1">j</kbd>{" "}
        <kbd className="rounded border border-b-2 border-border px-1">k</kbd> next / previous block
      </p>
    </div>
  );
}
