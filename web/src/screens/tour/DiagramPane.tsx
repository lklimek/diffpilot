import {
  Component,
  Suspense,
  lazy,
  useRef,
  useState,
  type ComponentType,
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
} from "react";
import type { Flow, StepId } from "../../api/generated/types";
import type { FlowDiagram } from "../../components/flow-diagram/FlowDiagram";
import { StatusMessage } from "../../components/status/StatusMessage";
import { Tooltip } from "../../components/tooltip/Tooltip";
import { DIAGRAM_MIN_PX } from "./layout";

export { DIAGRAM_MIN_PX };
/** Height change of one arrow key press. */
export const DIAGRAM_KEY_STEP_PX = 40;
/** Largest height: the viewport less room for the block below. */
export const diagramMaxPx = () => Math.max(DIAGRAM_MIN_PX, window.innerHeight - 160);
const clampHeight = (px: number) => Math.round(Math.min(diagramMaxPx(), Math.max(DIAGRAM_MIN_PX, px)));

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
  /** Dragged height in px (remembered with the layout); null = default. */
  height?: number | null;
  /** A new height from the handle; null resets to the default. */
  onHeight?: (height: number | null) => void;
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
  height = null,
  onHeight,
}: DiagramPaneProps) {
  const body = useRef<HTMLDivElement>(null);
  const drag = useRef<{ y: number; from: number } | null>(null);
  const current = () => height ?? body.current?.getBoundingClientRect().height ?? DIAGRAM_MIN_PX;
  const resize = (px: number | null) => onHeight?.(px === null ? null : clampHeight(px));
  const onKey = (event: KeyboardEvent<HTMLDivElement>) => {
    const next = {
      ArrowDown: current() + DIAGRAM_KEY_STEP_PX,
      ArrowUp: current() - DIAGRAM_KEY_STEP_PX,
      Home: DIAGRAM_MIN_PX,
      End: diagramMaxPx(),
    }[event.key];
    if (event.key === "Enter") resize(null);
    else if (next !== undefined) resize(next);
    else return;
    event.preventDefault();
  };
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
      <Tooltip
        tip={
          open
            ? "Hides the diagram; it stays hidden for the other flows until you show it again."
            : "Shows this flow's steps and branches as a diagram; click a step to open it."
        }
        className="block"
      >
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
      </Tooltip>
      {open && (
        <div
          ref={body}
          data-diagram-body
          style={height === null ? undefined : { height: `${height}px` }}
          className={`${height === null ? "max-h-[45vh]" : ""} overflow-auto border-t border-border px-3 py-2`}
        >
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
      {open && onHeight && (
        <Tooltip
          tip="Drag to make the diagram taller or shorter; arrow keys too, Home and End for the limits; double-click or Enter resets."
          className="block"
        >
          {/* A 24 px hit area over the bottom border; only a thin line shows. */}
          <div
            role="separator"
            aria-orientation="horizontal"
            aria-label="Resize the flow diagram"
            tabIndex={0}
            aria-valuenow={Math.round(height ?? DIAGRAM_MIN_PX)}
            aria-valuemin={DIAGRAM_MIN_PX}
            aria-valuemax={diagramMaxPx()}
            onPointerDown={(event: PointerEvent<HTMLDivElement>) => {
              drag.current = { y: event.clientY, from: current() };
              event.currentTarget.setPointerCapture?.(event.pointerId);
              event.preventDefault();
            }}
            onPointerMove={(event: PointerEvent<HTMLDivElement>) => {
              if (drag.current) resize(drag.current.from + event.clientY - drag.current.y);
            }}
            onPointerUp={() => (drag.current = null)}
            onPointerCancel={() => (drag.current = null)}
            onDoubleClick={() => resize(null)}
            onKeyDown={onKey}
            className="relative -mt-3 -mb-3 h-6 cursor-row-resize touch-none after:absolute after:inset-x-0 after:top-1/2 after:h-0.5 after:-translate-y-1/2 after:bg-transparent hover:after:bg-accent focus-visible:outline-none focus-visible:after:bg-accent"
          />
        </Tooltip>
      )}
    </section>
  );
}
