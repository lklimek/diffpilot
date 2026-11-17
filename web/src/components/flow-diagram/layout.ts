import { graphlib, layout as dagreLayout } from "@dagrejs/dagre";
import type { FlowGraph, NodeKind, StepId } from "../../api/generated/types";

/** Longest line of a node label, in characters. */
export const NODE_LABEL_CHARS = 24;
/** Longest line of a branch label, in characters. */
export const EDGE_LABEL_CHARS = 24;

// Text is measured by character count (no DOM): widths are generous averages of the 12 px / 11 px fonts.
const NODE_CHAR_WIDTH = 6.5;
const EDGE_CHAR_WIDTH = 7;
/** Line height of node text, px. */
export const NODE_LINE_HEIGHT = 16;
/** Line height of branch-label text, px. */
export const EDGE_LINE_HEIGHT = 14;
/** Horizontal and vertical padding inside a node, px. */
export const NODE_PADDING = { x: 10, y: 8 } as const;
/** How far the pointed ends of a branching step reach beyond its text box, px. */
export const BRANCH_INSET = 12;
const EDGE_LABEL_PADDING = 4;

/** Axis-aligned box; `x`, `y` is the top-left corner. */
export interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** A positioned step. */
export interface DiagramNode extends Box {
  stepId: StepId;
  number: number;
  kind: NodeKind;
  label: string;
  /** The label wrapped to lines. */
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
  points: { x: number; y: number }[];
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

/** Wraps `text` on spaces into lines of at most `maxChars`; longer words are split. */
export function wrapText(text: string, maxChars: number): string[] {
  const lines: string[] = [];
  let line = "";
  for (let word of text.trim().split(/\s+/).filter(Boolean)) {
    while (word.length > maxChars) {
      if (line) lines.push(line);
      lines.push(word.slice(0, maxChars));
      word = word.slice(maxChars);
      line = "";
    }
    if (!line) line = word;
    else if (line.length + 1 + word.length <= maxChars) line = `${line} ${word}`;
    else {
      lines.push(line);
      line = word;
    }
  }
  if (line || lines.length === 0) lines.push(line);
  return lines;
}

const longest = (lines: string[]) => Math.max(...lines.map((l) => l.length));

/** Lays out `graph` in `direction`; edges whose ends are not nodes of the graph are left out. */
export function layoutFlow(graph: FlowGraph, direction: Direction): DiagramLayout {
  const ids = new Set(graph.nodes.map((n) => n.step_id));
  const edges = graph.edges.filter((e) => ids.has(e.from) && ids.has(e.to));
  const outDegree = new Map<StepId, number>();
  for (const edge of edges) outDegree.set(edge.from, (outDegree.get(edge.from) ?? 0) + 1);

  const g = new graphlib.Graph({ multigraph: true });
  g.setGraph({ rankdir: direction, nodesep: 16, ranksep: 32, edgesep: 12, marginx: 8, marginy: 8 });
  const nodes = graph.nodes.map((node) => {
    const lines = wrapText(node.label, NODE_LABEL_CHARS);
    const prefixWidth = 22 + String(node.number).length * 8;
    const branching = (outDegree.get(node.step_id) ?? 0) >= 2;
    const width =
      2 * NODE_PADDING.x +
      prefixWidth +
      longest(lines) * NODE_CHAR_WIDTH +
      (branching ? 2 * BRANCH_INSET : 0);
    const height = 2 * NODE_PADDING.y + lines.length * NODE_LINE_HEIGHT;
    g.setNode(node.step_id, { width, height });
    return { node, lines, prefixWidth, branching };
  });
  const labelled = edges.map((edge, i) => {
    const labelLines = edge.label === null ? [] : wrapText(edge.label, EDGE_LABEL_CHARS);
    const key = String(i);
    g.setEdge(
      { v: edge.from, w: edge.to, name: key },
      edge.label === null
        ? { width: 0, height: 0 }
        : {
            width: longest(labelLines) * EDGE_CHAR_WIDTH + 2 * EDGE_LABEL_PADDING,
            height: labelLines.length * EDGE_LINE_HEIGHT + EDGE_LABEL_PADDING,
            labelpos: "c",
          },
    );
    return { edge, key, labelLines };
  });

  dagreLayout(g);

  return {
    direction,
    width: g.graph().width ?? 0,
    height: g.graph().height ?? 0,
    nodes: nodes.map(({ node, lines, prefixWidth, branching }) => {
      const { x, y, width, height } = g.node(node.step_id);
      return {
        stepId: node.step_id,
        number: node.number,
        kind: node.kind,
        label: node.label,
        lines,
        prefixWidth,
        branching,
        x: (x ?? 0) - width / 2,
        y: (y ?? 0) - height / 2,
        width,
        height,
      };
    }),
    edges: labelled.map(({ edge, key, labelLines }) => {
      const routed = g.edge({ v: edge.from, w: edge.to, name: key });
      const { width = 0, height = 0, x = 0, y = 0 } = routed;
      return {
        key,
        from: edge.from,
        to: edge.to,
        label: edge.label,
        labelLines,
        points: routed.points ?? [],
        labelBox: edge.label === null ? null : { x: x - width / 2, y: y - height / 2, width, height },
      };
    }),
  };
}

/** The left-to-right drawing when it fits `width` (uses wide screens), else top to bottom. */
export function pickLayout(layouts: Record<Direction, DiagramLayout>, width: number): DiagramLayout {
  return layouts.LR.width <= width ? layouts.LR : layouts.TB;
}
