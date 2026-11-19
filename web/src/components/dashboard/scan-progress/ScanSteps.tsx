import type { ScanStep, ScanStepKey, StepState } from "../../../api/generated/types";
import { Spinner } from "../common/icons";

const STEP: Record<ScanStepKey, [string, string]> = {
  search_prs: ["Search PRs", "authored, assigned, review requests"],
  issues: ["Issues", "watched repositories, assigned, mentions"],
  pr_details: ["PR details", "CI, threads, timeline"],
  context: ["Ownership & milestones", "owned paths, releases, reach"],
  history: ["History", "PRs and reviews for the charts"],
  todos: ["MemCan TODOs", "your projects"],
  ranking: ["LLM ranking", "changed and new items"],
};

const LOOK: Record<StepState, string> = {
  done: "before:mr-1 before:font-bold before:text-ok before:content-['✓']",
  running: "border-accent bg-accent-bg font-semibold",
  waiting: "text-muted",
  failed: "bg-high-bg font-semibold text-high before:mr-1 before:content-['✕']",
  skipped: "text-muted line-through",
};

function detail(step: ScanStep): string | null {
  const count = step.done !== null && step.total !== null ? `${step.done} / ${step.total}` : null;
  if (step.state !== "running" && step.state !== "failed") return null;
  return [count, step.detail].filter(Boolean).join(" · ") || null;
}

/** The scan steps in order with their state (mockup `.steps`); the state is also given as text. */
export function ScanSteps({ steps }: { steps: ScanStep[] }) {
  return (
    <ol className="m-0 flex list-none flex-wrap p-0">
      {steps.map((step) => {
        const [label, about] = STEP[step.key] ?? [String(step.key).replaceAll("_", " "), ""];
        return (
          <li
            key={step.key}
            data-step={step.key}
            data-state={step.state}
            className={`min-w-[100px] flex-1 border border-l-0 border-border bg-surface px-2.5 py-1.5 text-[12.5px] first:rounded-l-md first:border-l last:rounded-r-md ${LOOK[step.state] ?? ""}`}
          >
            {step.state === "running" && <Spinner />} {label}
            <span className="sr-only"> ({step.state})</span>
            <small className="block text-[11px] font-normal text-muted">{detail(step) ?? about}</small>
          </li>
        );
      })}
    </ol>
  );
}
