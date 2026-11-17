import { Component, Suspense, lazy, useState, type ComponentType, type ReactNode } from "react";
import type { Flow, StepId } from "../../api/generated/types";
import type { FlowDiagram } from "../../components/flow-diagram/FlowDiagram";
import { StatusMessage } from "../../components/status/StatusMessage";
import { Tooltip } from "../../components/tooltip/Tooltip";

type DiagramComponent = ComponentType<Parameters<typeof FlowDiagram>[0]>;

/** Loads the diagram chunk (own chunk: the layout library is large). */
const loadDiagram = () =>
  import("../../components/flow-diagram/FlowDiagram").then((m) => ({ default: m.FlowDiagram }));

interface DiagramPaneProps {
  flow: Flow;
  currentStepId: StepId | null;
  onSelectStep: (step: StepId) => void;
  /** Whether the diagram is shown (remembered by the screen). */
  open: boolean;
  onToggle: () => void;
  /** The diagram module; the app's chunk by default. */
  load?: () => Promise<{ default: DiagramComponent }>;
}

/** Catches a failed diagram chunk so the rest of the review stays. */
class LoadBoundary extends Component<{ fallback: ReactNode; children: ReactNode }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    return this.state.failed ? this.props.fallback : this.props.children;
  }
}

/** The flow's steps and branches as text: what the diagram shows, without its chunk. */
function StepList({ flow, onSelectStep }: Pick<DiagramPaneProps, "flow" | "onSelectStep">) {
  const nodes = [...flow.graph.nodes].sort((a, b) => a.number - b.number);
  const numberOf = new Map(nodes.map((n) => [n.step_id, n.number]));
  return (
    <ol className="m-0 grid list-none gap-0.5 p-0 text-[12.5px]">
      {nodes.map((node) => {
        const next = flow.graph.edges
          .filter((e) => e.from === node.step_id && numberOf.has(e.to))
          .map((e) => `step ${numberOf.get(e.to)}${e.label ? ` (${e.label})` : ""}`);
        return (
          <li key={node.step_id}>
            <button
              type="button"
              onClick={() => onSelectStep(node.step_id)}
              className="min-h-6 cursor-pointer text-left text-accent underline"
            >
              Step {node.number}: {node.label}
            </button>
            {next.length > 0 && <span className="text-muted"> → {next.join(", ")}</span>}
          </li>
        );
      })}
    </ol>
  );
}

/** The flow-diagram pane above the block: hidden by default, loaded on first show, text if loading fails. */
export function DiagramPane({
  flow,
  currentStepId,
  onSelectStep,
  open,
  onToggle,
  load = loadDiagram,
}: DiagramPaneProps) {
  // A rejected lazy import stays rejected: Retry makes a new one.
  const [diagram, setDiagram] = useState(() => ({ attempt: 0, Diagram: lazy(load) }));
  const { Diagram } = diagram;
  const failed = (
    <div className="grid gap-1.5">
      <p role="alert" className="text-[12.5px]">
        The diagram could not be loaded. The steps of this flow and where each goes next:{" "}
        <Tooltip tip="Tries to load the diagram again.">
          <button
            type="button"
            onClick={() => setDiagram((d) => ({ attempt: d.attempt + 1, Diagram: lazy(load) }))}
            className="cursor-pointer rounded border border-border bg-surface px-2 text-text hover:border-accent"
          >
            Retry
          </button>
        </Tooltip>
      </p>
      <StepList flow={flow} onSelectStep={onSelectStep} />
    </div>
  );
  return (
    <section aria-label="Flow diagram" className="mb-3 rounded-lg border border-border bg-surface">
      <button
        type="button"
        aria-expanded={open}
        onClick={onToggle}
        className="flex w-full cursor-pointer items-center gap-2 border-0 bg-transparent px-3 py-1.5 text-left text-[12.5px] font-semibold text-text"
      >
        <span aria-hidden>{open ? "▾" : "▸"}</span>
        {open ? "Hide flow diagram" : "Show flow diagram"}
        <span className="font-normal text-muted">
          Flow {flow.position}: {flow.llm.title}
        </span>
      </button>
      {open && (
        <div className="max-h-[45vh] overflow-auto border-t border-border px-3 py-2">
          <LoadBoundary key={diagram.attempt} fallback={failed}>
            <Suspense fallback={<StatusMessage kind="loading">Loading the diagram…</StatusMessage>}>
              <Diagram
                graph={flow.graph}
                decisions={Object.fromEntries(flow.steps.map((s) => [s.id, s.llm.decides]))}
                title={flow.llm.title}
                currentStepId={currentStepId}
                onSelectStep={onSelectStep}
              />
            </Suspense>
          </LoadBoundary>
        </div>
      )}
    </section>
  );
}
