import { graphlib, layout as dagreLayout } from "@dagrejs/dagre";
import type { FlowGraph, GraphEdge, GraphNode, NodeKind, StepId } from "../../api/generated/types";
import { estimateWidth, type Measure } from "./measure";

// Caps: labels are LLM text without a length limit in the schema; the drawing stays bounded.
/** Widest line of a node label, px. */
export const NODE_TEXT_WIDTH = 160;
/** Widest line of a branch label, px. */
export const EDGE_TEXT_WIDTH = 150;
/** Most lines drawn for a node label; more end in "…". */
export const NODE_MAX_LINES = 5;
/** Most lines drawn for a branch label; more end in "…". */
export const EDGE_MAX_LINES = 3;
/** Most characters of a node label that are drawn. */
export const LABEL_MAX_CHARS = 160;
const EDGE_LABEL_MAX_CHARS = 80;
/** Most characters of a label in accessible names and the text alternative. */
export const TEXT_MAX_CHARS = 1000;
/** Largest flow that is drawn; larger flows are shown only as text (real flows have up to about 30 steps). */
export const MAX_DRAWN_STEPS = 40;
/** Most edges of a flow that is drawn (layout cost grows fast with long, crossing edges). */
export const MAX_DRAWN_EDGES = 80;

/** Most steps listed in the text alternative. */
export const MAX_LISTED_STEPS = 500;
/** Most outgoing edges listed per step in the text alternative. */
export const MAX_LISTED_EDGES = 20;
const LISTED_EDGE_LABEL_CHARS = 200;

/** Line height of node text, px. */
export const NODE_LINE_HEIGHT = 16;
/** Line height of branch-label text, px. */
export const EDGE_LINE_HEIGHT = 14;
/** Horizontal and vertical padding inside a node, px. */
export const NODE_PADDING = { x: 10, y: 8 } as const;
/** How far the pointed ends of a branching step reach beyond its text box, px. */
export const BRANCH_INSET = 12;
/** Space for the kind glyph in front of the step number, px. */
export const GLYPH_WIDTH = 14;
const EDGE_LABEL_PADDING = 4;
const LOOP_GAP = 14;
const LOOP_STEP = 8;
const ELLIPSIS = "…";

/** Axis-aligned box; `x`, `y` is the top-left corner. */
export interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

type Point = { x: number; y: number };

/** A positioned step. */
export interface DiagramNode extends Box {
  stepId: StepId;
  number: number;
  kind: NodeKind;
  /** The label, cut to `TEXT_MAX_CHARS`. */
  label: string;
  /** The drawn label wrapped to lines. */
  lines: string[];
  /** Width of the glyph and number in front of the label, px. */
  prefixWidth: number;
  /** Two or more outgoing edges (drawn with pointed ends). */
  branching: boolean;
}

/** A routed edge. */
export interface DiagramEdge {
  /** Unique within the layout (parallel edges share `from` and `to`). */
  key: string;
  from: StepId;
  to: StepId;
  label: string | null;
  labelLines: string[];
  points: Point[];
  /** Where the branch label goes; null without a label. */
  labelBox: Box | null;
}

/** Drawing direction: top to bottom or left to right. */
export type Direction = "TB" | "LR";

/** A laid-out flow graph. */
export interface DiagramLayout {
  direction: Direction;
  width: number;
  height: number;
  nodes: DiagramNode[];
  edges: DiagramEdge[];
}

/** `text` cut to `max` UTF-16 units with "…" when longer; never splits a surrogate pair. */
export function boundText(text: string, max: number): string {
  if (text.length <= max) return text;
  const cut = text.slice(0, max);
  const last = cut.charCodeAt(cut.length - 1);
  return `${last >= 0xd800 && last <= 0xdbff ? cut.slice(0, -1) : cut}${ELLIPSIS}`;
}

/** Whether `graph` is small enough to lay out and draw. */
export function drawable(graph: FlowGraph): boolean {
  return graph.nodes.length <= MAX_DRAWN_STEPS && graph.edges.length <= MAX_DRAWN_EDGES;
}

const widest = (values: number[]) => values.reduce((a, b) => Math.max(a, b), 0);

const RANK_SEP = 32;
const MARGIN = 8;

/** Drawn lines and size of a step's node. */
function sizeNode(node: GraphNode, branching: boolean, measure: Measure) {
  const nodeWidth = (s: string) => measure(s, "node");
  const lines = wrapText(boundText(node.label, LABEL_MAX_CHARS), NODE_TEXT_WIDTH, nodeWidth, NODE_MAX_LINES);
  const prefixWidth = GLYPH_WIDTH + measure(String(node.number), "number") + 6;
  const width =
    2 * NODE_PADDING.x + prefixWidth + widest(lines.map(nodeWidth)) + (branching ? 2 * BRANCH_INSET : 0);
  const height = 2 * NODE_PADDING.y + lines.length * NODE_LINE_HEIGHT;
  return { lines, prefixWidth, width, height };
}

/** Edges between nodes of `graph` and the number of outgoing edges per step. */
function validEdges(graph: FlowGraph) {
  const ids = new Set(graph.nodes.map((n) => n.step_id));
  const edges = graph.edges.filter((e) => ids.has(e.from) && ids.has(e.to));
  const outDegree = new Map<StepId, number>();
  for (const edge of edges) outDegree.set(edge.from, (outDegree.get(edge.from) ?? 0) + 1);
  return { edges, outDegree };
}

/**
 * Cheap estimate of the left-to-right drawing's width without laying it out, normally below the real width:
 * the widest chain of nodes along edges (back edges of a depth-first search left out) with the rank gaps.
 */
export function estimateLeftToRightWidth(graph: FlowGraph, measure: Measure): number {
  const { edges, outDegree } = validEdges(graph);
  const width = new Map(
    graph.nodes.map((n) => [n.step_id, sizeNode(n, (outDegree.get(n.step_id) ?? 0) >= 2, measure).width]),
  );
  const next = new Map<StepId, StepId[]>();
  for (const e of edges) if (e.from !== e.to) next.set(e.from, [...(next.get(e.from) ?? []), e.to]);
  const state = new Map<StepId, "open" | "done">();
  const chain = new Map<StepId, number>();
  const visit = (id: StepId) => {
    state.set(id, "open");
    let best = 0;
    for (const to of next.get(id) ?? []) {
      if (!state.has(to)) visit(to);
      if (state.get(to) === "done") best = Math.max(best, chain.get(to)! + RANK_SEP);
    }
    chain.set(id, (width.get(id) ?? 0) + best);
    state.set(id, "done");
  };
  for (const node of graph.nodes) if (!state.has(node.step_id)) visit(node.step_id);
  return widest([...chain.values()]) + 2 * MARGIN;
}

/** Whether a word may break after `chars[0..n)`: after `_`, `.`, `/` or a whole `::`. */
function breaksAfter(chars: readonly string[], n: number): boolean {
  const last = chars[n - 1];
  if (last === "_" || last === "." || last === "/") return true;
  return last === ":" && chars[n - 2] === ":" && chars[n] !== ":";
}

/**
 * Wraps bounded `text` on spaces into lines no wider than `maxWidth`. Wider words are split after `_`, `::`,
 * `.` or `/` where possible, else at the last fitting character (never inside `::`). Beyond `maxLines` the last
 * line ends in "…".
 */
export function wrapText(
  text: string,
  maxWidth: number,
  width: (s: string) => number,
  maxLines: number,
): string[] {
  const lines: string[] = [];
  let line = "";
  for (const word of text.trim().split(/\s+/).filter(Boolean)) {
    let chars = [...word];
    while (width(chars.join("")) > maxWidth) {
      if (line) lines.push(line);
      line = "";
      let n = 1;
      while (n < chars.length && width(chars.slice(0, n + 1).join("")) <= maxWidth) n += 1;
      let cut = n;
      while (cut > 1 && !breaksAfter(chars, cut)) cut -= 1;
      if (cut > 1) n = cut;
      else if (n > 1 && chars[n - 1] === ":" && chars[n] === ":") n -= 1;
      lines.push(chars.slice(0, n).join(""));
      chars = chars.slice(n);
    }
    const rest = chars.join("");
    if (!rest) continue;
    if (!line) line = rest;
    else if (width(`${line} ${rest}`) <= maxWidth) line = `${line} ${rest}`;
    else {
      lines.push(line);
      line = rest;
    }
  }
  if (line || lines.length === 0) lines.push(line);
  if (lines.length <= maxLines) return lines;
  let last = [...lines[maxLines - 1]!];
  while (last.length > 0 && width(`${last.join("").trimEnd()}${ELLIPSIS}`) > maxWidth)
    last = last.slice(0, -1);
  return [...lines.slice(0, maxLines - 1), `${last.join("").trimEnd()}${ELLIPSIS}`];
}

/** Where the line from `from` to the centre of `box` crosses the border of `box`. */
function clipToBox(box: Box, from: Point): Point {
  const cx = box.x + box.width / 2;
  const cy = box.y + box.height / 2;
  const dx = from.x - cx;
  const dy = from.y - cy;
  if (dx === 0 && dy === 0) return { x: cx, y: box.y };
  if (Math.abs(dy) * box.width > Math.abs(dx) * box.height) {
    const sy = dy < 0 ? -box.height / 2 : box.height / 2;
    return { x: cx + (sy * dx) / dy, y: cy + sy };
  }
  const sx = dx < 0 ? -box.width / 2 : box.width / 2;
  return { x: cx + sx, y: cy + (sx * dy) / dx };
}

/** Lays out `graph` in `direction`; edges whose ends are not nodes of the graph are left out. */
export function layoutFlow(
  graph: FlowGraph,
  direction: Direction,
  measure: Measure = estimateWidth,
): DiagramLayout {
  const edgeWidth = (s: string) => measure(s, "edge");
  const { edges, outDegree } = validEdges(graph);

  const prepared = edges.map((edge: GraphEdge, i) => {
    const lines =
      edge.label === null
        ? []
        : wrapText(boundText(edge.label, EDGE_LABEL_MAX_CHARS), EDGE_TEXT_WIDTH, edgeWidth, EDGE_MAX_LINES);
    const size =
      edge.label === null
        ? { width: 0, height: 0 }
        : {
            width: widest(lines.map(edgeWidth)) + 2 * EDGE_LABEL_PADDING,
            height: lines.length * EDGE_LINE_HEIGHT + EDGE_LABEL_PADDING,
          };
    return {
      edge,
      key: String(i),
      lines,
      size,
      label: edge.label === null ? null : boundText(edge.label, TEXT_MAX_CHARS),
    };
  });
  // Self-loops are drawn here, not by dagre (3.1.3 returns them detached from the node): the node's layout
  // box is enlarged around its centre to hold the loops and their labels.
  const loopsOf = new Map<StepId, typeof prepared>();
  for (const item of prepared) {
    if (item.edge.from === item.edge.to)
      loopsOf.set(item.edge.from, [...(loopsOf.get(item.edge.from) ?? []), item]);
  }

  const g = new graphlib.Graph({ multigraph: true });
  g.setGraph({
    rankdir: direction,
    nodesep: 16,
    ranksep: RANK_SEP,
    edgesep: 12,
    marginx: MARGIN,
    marginy: MARGIN,
  });
  const nodes = graph.nodes.map((node) => {
    const branching = (outDegree.get(node.step_id) ?? 0) >= 2;
    const { lines, prefixWidth, width, height } = sizeNode(node, branching, measure);
    const loops = loopsOf.get(node.step_id) ?? [];
    const reach = loops.length === 0 ? 0 : LOOP_GAP + LOOP_STEP * (loops.length - 1);
    const labels = loops.filter((l) => l.label !== null).map((l) => l.size);
    const labelsWidth = widest(labels.map((l) => l.width));
    const labelsHeight = labels.reduce((sum, l) => sum + l.height + 2, 0);
    const half =
      loops.length === 0
        ? { w: width / 2, h: height / 2 }
        : direction === "TB"
          ? { w: width / 2 + reach + 4 + labelsWidth, h: Math.max(height / 2 + reach, labelsHeight / 2) }
          : { w: Math.max(width / 2, labelsWidth / 2), h: height / 2 + reach + 4 + labelsHeight };
    g.setNode(node.step_id, { width: 2 * half.w, height: 2 * half.h });
    return { node, lines, prefixWidth, branching, width, height, loops, reach, labelsHeight };
  });
  for (const { edge, key, size, label } of prepared) {
    if (edge.from !== edge.to) {
      g.setEdge({ v: edge.from, w: edge.to, name: key }, label === null ? size : { ...size, labelpos: "c" });
    }
  }

  dagreLayout(g);

  const routed = new Map<string, DiagramEdge>();
  const laidOut = nodes.map(
    ({ node, lines, prefixWidth, branching, width, height, loops, reach, labelsHeight }) => {
      const centre = g.node(node.step_id);
      const box: DiagramNode = {
        stepId: node.step_id,
        number: node.number,
        kind: node.kind,
        label: boundText(node.label, TEXT_MAX_CHARS),
        lines,
        prefixWidth,
        branching,
        x: (centre.x ?? 0) - width / 2,
        y: (centre.y ?? 0) - height / 2,
        width,
        height,
      };
      const { x, y } = box;
      let labelAt = direction === "TB" ? (centre.y ?? 0) - labelsHeight / 2 : y + height + reach + 4;
      loops.forEach(({ edge, key, lines: labelLines, size, label }, i) => {
        const o = LOOP_GAP + LOOP_STEP * i;
        let points: Point[];
        if (direction === "TB") {
          const xs = Math.max(x + width / 2, x + width - BRANCH_INSET - 8 - 6 * i);
          points = [
            { x: xs, y },
            { x: xs, y: y - o },
            { x: x + width + o, y: y - o },
            { x: x + width + o, y: y + height + o },
            { x: xs, y: y + height + o },
            { x: xs, y: y + height },
          ];
        } else {
          const inset = Math.min(BRANCH_INSET + 8 + 6 * i, width * 0.4);
          points = [
            { x: x + inset, y: y + height },
            { x: x + inset, y: y + height + o },
            { x: x + width - inset, y: y + height + o },
            { x: x + width - inset, y: y + height },
          ];
        }
        let labelBox: Box | null = null;
        if (label !== null) {
          labelBox =
            direction === "TB"
              ? { x: x + width + reach + 4, y: labelAt, ...size }
              : { x: (centre.x ?? 0) - size.width / 2, y: labelAt, ...size };
          labelAt += size.height + 2;
        }
        routed.set(key, { key, from: edge.from, to: edge.to, label, labelLines, points, labelBox });
      });
      return box;
    },
  );

  const byId = new Map(laidOut.map((n) => [n.stepId, n]));
  return {
    direction,
    width: g.graph().width ?? 0,
    height: g.graph().height ?? 0,
    nodes: laidOut,
    edges: prepared.map(({ edge, key, lines, label }) => {
      const loop = routed.get(key);
      if (loop) return loop;
      const routedEdge = g.edge({ v: edge.from, w: edge.to, name: key });
      const points = [...(routedEdge.points ?? [])];
      // Ends at an enlarged layout box (a step with self-loops) are moved onto the drawn node.
      if (points.length >= 2) {
        if (loopsOf.has(edge.from)) points[0] = clipToBox(byId.get(edge.from)!, points[1]!);
        if (loopsOf.has(edge.to))
          points[points.length - 1] = clipToBox(byId.get(edge.to)!, points[points.length - 2]!);
      }
      const { width = 0, height = 0, x = 0, y = 0 } = routedEdge;
      return {
        key,
        from: edge.from,
        to: edge.to,
        label,
        labelLines: lines,
        points,
        labelBox: label === null ? null : { x: x - width / 2, y: y - height / 2, width, height },
      };
    }),
  };
}

/**
 * The drawing for a column `width` px wide: left to right when it fits (wide screens), else top to bottom.
 * `get` lays a direction out; left to right is only tried when the cheap estimate fits.
 */
export function layoutForWidth(
  width: number,
  estimatedLeftToRight: number,
  get: (direction: Direction) => DiagramLayout,
): DiagramLayout {
  if (width > 0 && estimatedLeftToRight <= width) {
    const leftToRight = get("LR");
    if (leftToRight.width <= width) return leftToRight;
  }
  return get("TB");
}

/** One step of the text alternative. */
export interface StepText {
  stepId: StepId;
  number: number;
  kind: NodeKind;
  /** The label, cut to `TEXT_MAX_CHARS`. */
  label: string;
  branching: boolean;
  /** Outgoing edges, at most `MAX_LISTED_EDGES`: "step 4 (Ok)". */
  next: string[];
  /** Outgoing edges not in `next`. */
  moreNext: number;
}

/** The steps of `graph` in number order with their outgoing edges, for the text alternative. */
export function describeSteps(graph: FlowGraph): StepText[] {
  const numberOf = new Map(graph.nodes.map((n) => [n.step_id, n.number]));
  const outgoing = new Map<StepId, GraphEdge[]>();
  for (const edge of graph.edges) {
    if (!numberOf.has(edge.from) || !numberOf.has(edge.to)) continue;
    const list = outgoing.get(edge.from);
    if (list) list.push(edge);
    else outgoing.set(edge.from, [edge]);
  }
  return [...graph.nodes]
    .sort((a, b) => a.number - b.number)
    .map((node) => {
      const edges = outgoing.get(node.step_id) ?? [];
      return {
        stepId: node.step_id,
        number: node.number,
        kind: node.kind,
        label: boundText(node.label, TEXT_MAX_CHARS),
        branching: edges.length >= 2,
        next: edges
          .slice(0, MAX_LISTED_EDGES)
          .map(
            (e) =>
              `step ${numberOf.get(e.to)}${e.label === null ? "" : ` (${boundText(e.label, LISTED_EDGE_LABEL_CHARS)})`}`,
          ),
        moreNext: Math.max(0, edges.length - MAX_LISTED_EDGES),
      };
    });
}
