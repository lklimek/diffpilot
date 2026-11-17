import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import type { FlowGraph, NodeKind, StepId } from "../../api/generated/types";
import {
  BRANCH_INSET,
  EDGE_LINE_HEIGHT,
  GLYPH_WIDTH,
  MAX_LISTED_STEPS,
  NODE_LINE_HEIGHT,
  NODE_PADDING,
  describeSteps,
  drawable,
  estimateLeftToRightWidth,
  layoutFlow,
  layoutForWidth,
  type DiagramLayout,
  type DiagramNode,
  type Direction,
} from "./layout";
import { canvasMeasure, estimateWidth } from "./measure";

interface FlowDiagramProps {
  /** The flow graph (one node per step). */
  graph: FlowGraph;
  /** Flow title, used in the accessible name. */
  title: string;
  /** The step shown in the centre; null when the current block is not a step of this flow. */
  currentStepId: StepId | null;
  /** Called with the step of a clicked or keyboard-selected node. */
  onSelectStep: (stepId: StepId) => void;
}

// Kind = colour AND border style AND glyph (brief §4.3: never colour alone).
const KIND: Record<NodeKind, { glyph: string; word: string; look: string; dash?: string }> = {
  added: { glyph: "+", word: "added", look: "fill-add-bg stroke-add-bd stroke-[2.5]" },
  changed: { glyph: "~", word: "changed", look: "fill-chg-bg stroke-chg-bd stroke-2", dash: "7 3" },
  unchanged: {
    glyph: "=",
    word: "unchanged context",
    look: "fill-surface-2 stroke-mv-bd stroke-2",
    dash: "2 3",
  },
};
const FOCUS_LOOK =
  "group-hover:stroke-accent group-focus-visible:stroke-accent group-focus-visible:stroke-[3.5]";

const nodeName = (n: Pick<DiagramNode, "number" | "kind" | "branching" | "label">) =>
  `Step ${n.number}, ${KIND[n.kind].word}${n.branching ? ", branching" : ""}: ${n.label}`;

const hexagon = ({ x, y, width: w, height: h }: DiagramNode) =>
  [
    [x, y + h / 2],
    [x + BRANCH_INSET, y],
    [x + w - BRANCH_INSET, y],
    [x + w, y + h / 2],
    [x + w - BRANCH_INSET, y + h],
    [x + BRANCH_INSET, y + h],
  ]
    .map((p) => p.join(","))
    .join(" ");

/** Lines as tspans; a trailing space keeps words apart in the text content. */
const lineSpans = (lines: string[], x: number, lineHeight: number) =>
  lines.map((line, i) => (
    <tspan key={i} x={x} dy={i === 0 ? 0 : lineHeight}>
      {i < lines.length - 1 ? `${line} ` : line}
    </tspan>
  ));

/** Flow diagram: numbered steps, branch labels, current step; click or Enter on a node selects its step. */
export function FlowDiagram({ graph, title, currentStepId, onSelectStep }: FlowDiagramProps) {
  const arrowId = `flow-arrow-${useId().replace(/[^\w-]/g, "")}`;
  const scroller = useRef<HTMLDivElement>(null);
  const nodeRefs = useRef(new Map<StepId, SVGGElement>());
  const [width, setWidth] = useState(0);
  // Per graph: the text measure (page font when a canvas is available; its cache lives with this graph) and
  // each direction's layout, computed only when that direction is shown.
  const drawing = useMemo(() => {
    if (!drawable(graph)) return null;
    const measure =
      canvasMeasure(getComputedStyle(document.body).fontFamily || "sans-serif") ?? estimateWidth;
    const layouts = new Map<Direction, DiagramLayout>();
    const get = (direction: Direction) => {
      let layout = layouts.get(direction);
      if (!layout) {
        layout = layoutFlow(graph, direction, measure);
        layouts.set(direction, layout);
      }
      return layout;
    };
    return { get, estimate: estimateLeftToRightWidth(graph, measure) };
  }, [graph]);
  const layout = drawing && layoutForWidth(width, drawing.estimate, drawing.get);
  const ordered = useMemo(
    () => (layout ? [...layout.nodes].sort((a, b) => a.number - b.number) : []),
    [layout],
  );
  const steps = useMemo(() => describeSteps(graph), [graph]);

  // Roving tab stop: follows the current step, then the user's arrow keys.
  const [focusedId, setFocusedId] = useState(currentStepId);
  const [lastCurrent, setLastCurrent] = useState(currentStepId);
  if (currentStepId !== lastCurrent) {
    setLastCurrent(currentStepId);
    setFocusedId(currentStepId);
  }
  const tabStop = ordered.some((n) => n.stepId === focusedId) ? focusedId : (ordered[0]?.stepId ?? null);
  const empty = graph.nodes.length === 0;

  useLayoutEffect(() => {
    const element = scroller.current;
    if (!element || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(([entry]) => setWidth(entry?.contentRect.width ?? 0));
    observer.observe(element);
    return () => observer.disconnect();
  }, [empty]);

  // Keep the current step in view inside the diagram's own scroll area.
  useEffect(() => {
    const element = scroller.current;
    const node = currentStepId === null ? undefined : nodeRefs.current.get(currentStepId);
    if (!element || !node) return;
    const view = element.getBoundingClientRect();
    const box = node.getBoundingClientRect();
    const outside =
      box.top < view.top || box.bottom > view.bottom || box.left < view.left || box.right > view.right;
    if (outside) {
      element.scrollBy({
        top: box.top - view.top - (view.height - box.height) / 2,
        left: box.left - view.left - (view.width - box.width) / 2,
      });
    }
  }, [currentStepId, layout]);

  if (empty) return <p className="text-[13px] text-muted">This flow has no steps to draw.</p>;

  const onKeyDown = (event: KeyboardEvent<SVGGElement>, index: number) => {
    const moves: Record<string, number> = {
      ArrowDown: index + 1,
      ArrowRight: index + 1,
      ArrowUp: index - 1,
      ArrowLeft: index - 1,
      Home: 0,
      End: ordered.length - 1,
    };
    const node = ordered[index]!;
    const move = moves[event.key];
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      onSelectStep(node.stepId);
    } else if (move !== undefined) {
      event.preventDefault();
      const target = ordered[Math.max(0, Math.min(ordered.length - 1, move))]!;
      setFocusedId(target.stepId);
      nodeRefs.current.get(target.stepId)?.focus();
    }
  };

  const listed = steps.slice(0, MAX_LISTED_STEPS);
  const textAlternative = (
    <details open={!layout} className="mt-2 text-[13px]">
      <summary className="cursor-pointer text-muted">Diagram as text</summary>
      <ol aria-label="Flow steps and edges" className="m-0 mt-1 grid list-none gap-1 p-0">
        {listed.map((step) => {
          const more = step.moreNext > 0 ? ` and ${step.moreNext} more` : "";
          const next = step.next.length === 0 ? "no outgoing edge" : `goes to ${step.next.join(", ")}${more}`;
          return <li key={step.stepId}>{`${nodeName(step)} — ${next}`}</li>;
        })}
      </ol>
      {steps.length > listed.length && (
        <p className="mt-1 text-muted">{`${steps.length - listed.length} more steps are not listed.`}</p>
      )}
    </details>
  );

  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      {layout ? <Legend /> : null}
      <div ref={scroller} className="min-h-0 flex-1 overflow-auto">
        {!layout && (
          <p className="text-[13px] text-muted">
            {`This flow has ${graph.nodes.length} steps and ${graph.edges.length} edges: too many to draw. The list below shows it as text.`}
          </p>
        )}
        {layout && (
          <svg
            role="group"
            aria-label={`Flow diagram: ${title}, ${ordered.length} ${ordered.length === 1 ? "step" : "steps"}`}
            width={layout.width}
            height={layout.height}
            data-direction={layout.direction}
            viewBox={`0 0 ${layout.width} ${layout.height}`}
            className="mx-auto block max-w-none"
          >
            <defs>
              <marker
                id={arrowId}
                viewBox="0 0 10 10"
                refX="9"
                refY="5"
                markerWidth="7"
                markerHeight="7"
                orient="auto-start-reverse"
              >
                <path d="M0,0 L10,5 L0,10 z" className="fill-muted" />
              </marker>
            </defs>
            {layout.edges.map((edge) => (
              <path
                key={edge.key}
                data-edge
                d={edge.points.map((p, i) => `${i === 0 ? "M" : "L"}${p.x},${p.y}`).join(" ")}
                markerEnd={`url(#${arrowId})`}
                className="fill-none stroke-muted stroke-[1.6]"
              />
            ))}
            {layout.edges.map(
              ({ key, labelBox: box, labelLines }) =>
                box && (
                  <g key={key} data-edge-label>
                    <rect {...box} rx={3} className="fill-surface opacity-90" />
                    <text
                      x={box.x + box.width / 2}
                      y={box.y + 13}
                      textAnchor="middle"
                      className="fill-text text-[11px] font-semibold"
                    >
                      {lineSpans(labelLines, box.x + box.width / 2, EDGE_LINE_HEIGHT)}
                    </text>
                  </g>
                ),
            )}
            {ordered.map((node, index) => {
              const kind = KIND[node.kind];
              const current = node.stepId === currentStepId;
              const textX = node.x + NODE_PADDING.x + (node.branching ? BRANCH_INSET : 0);
              const baseline = node.y + NODE_PADDING.y + 12;
              const shape = `${kind.look} ${FOCUS_LOOK}`;
              return (
                <g
                  key={node.stepId}
                  ref={(element) => {
                    if (element) nodeRefs.current.set(node.stepId, element);
                    else nodeRefs.current.delete(node.stepId);
                  }}
                  role="button"
                  tabIndex={node.stepId === tabStop ? 0 : -1}
                  aria-label={nodeName(node)}
                  aria-current={current ? "step" : undefined}
                  data-step={node.stepId}
                  data-kind={node.kind}
                  className="group cursor-pointer outline-none"
                  onClick={() => onSelectStep(node.stepId)}
                  onFocus={() => setFocusedId(node.stepId)}
                  onKeyDown={(event) => onKeyDown(event, index)}
                >
                  {current && (
                    <rect
                      data-current-outline
                      x={node.x - 5}
                      y={node.y - 5}
                      width={node.width + 10}
                      height={node.height + 10}
                      rx={9}
                      className="fill-none stroke-accent stroke-[2.5]"
                    />
                  )}
                  {node.branching ? (
                    <polygon
                      data-shape
                      points={hexagon(node)}
                      strokeDasharray={kind.dash}
                      className={shape}
                    />
                  ) : (
                    <rect
                      data-shape
                      x={node.x}
                      y={node.y}
                      width={node.width}
                      height={node.height}
                      rx={6}
                      strokeDasharray={kind.dash}
                      className={shape}
                    />
                  )}
                  <text
                    data-glyph
                    x={textX}
                    y={baseline}
                    className="fill-text font-mono text-[13px] font-bold"
                  >
                    {kind.glyph}
                  </text>
                  <text x={textX + GLYPH_WIDTH} y={baseline} className="fill-text text-[12px] font-bold">
                    {` ${node.number} `}
                  </text>
                  <text x={textX + node.prefixWidth} y={baseline} className="fill-text text-[12px]">
                    {lineSpans(node.lines, textX + node.prefixWidth, NODE_LINE_HEIGHT)}
                  </text>
                </g>
              );
            })}
          </svg>
        )}
        {textAlternative}
      </div>
    </div>
  );
}

const SWATCH = "inline-block h-3 w-6 align-[-2px] mr-1.5 overflow-visible";

/** Legend of the node drawing. */
function Legend() {
  return (
    <ul
      aria-label="Diagram legend"
      className="m-0 flex list-none flex-wrap gap-x-3.5 gap-y-1 p-0 text-[11.5px] text-muted"
    >
      {(Object.keys(KIND) as NodeKind[]).map((key) => (
        <li key={key}>
          <svg aria-hidden className={SWATCH}>
            <rect
              x="1"
              y="1"
              width="22"
              height="10"
              rx="2"
              strokeDasharray={KIND[key].dash}
              className={KIND[key].look}
            />
          </svg>
          {`${KIND[key].glyph} ${KIND[key].word}`}
        </li>
      ))}
      <li>
        <svg aria-hidden className={SWATCH}>
          <polygon points="1,6 6,1 18,1 23,6 18,11 6,11" className="fill-surface stroke-mv-bd stroke-2" />
        </svg>
        branching step (two or more outgoing edges)
      </li>
      <li>
        <svg aria-hidden className={SWATCH}>
          <rect x="1" y="1" width="22" height="10" rx="3" className="fill-none stroke-accent stroke-[2.5]" />
        </svg>
        current step
      </li>
      <li>Step kinds are LLM analysis.</li>
    </ul>
  );
}
