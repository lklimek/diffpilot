// Static preview of the flow diagram alone (fictional fixture graphs): `npm run build:diagram-preview`.
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import type { FlowGraph, StepId } from "../api/generated/types";
import { FlowDiagram } from "../components/flow-diagram/FlowDiagram";
import graphs from "../mocks/diagrams/large-flow-graphs.json";
import "./diagram-preview.css";

function Preview() {
  const [current, setCurrent] = useState<StepId | null>(null);
  return (
    <main className="grid gap-6 bg-bg p-6 text-text">
      <h1 className="text-lg font-bold">Flow diagram preview (fictional data)</h1>
      {Object.entries(graphs as Record<string, FlowGraph>).map(([name, graph]) => (
        <section
          key={name}
          aria-label={`${name} flow`}
          className="rounded-lg border border-border bg-surface p-3"
        >
          <h2 className="mb-2 text-sm font-semibold">
            {name}: {graph.nodes.length} steps, {graph.edges.length} edges
          </h2>
          <FlowDiagram graph={graph} title={name} currentStepId={current} onSelectStep={setCurrent} />
        </section>
      ))}
    </main>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Preview />
  </StrictMode>,
);
