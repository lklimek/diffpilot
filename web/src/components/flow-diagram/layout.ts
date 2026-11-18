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
/** Distance of the first side lane from the node column, px. */
export const LANE_FIRST = 32;
/** Distance between parallel side lanes, px. */
export const LANE_STEP = 14;
/** Distance between edge ends on one side of a node, px. */
const ATTACH_STEP = 6;
/** Side offset of parallel edges between neighbouring steps, px. */
const PARALLEL_STEP = 10;

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
    acyclicer: "greedy",
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
    g.setNode(node.step_id, { number: node.number, width: 2 * half.w, height: 2 * half.h });
    return { node, lines, prefixWidth, branching, width, height, loops, reach, labelsHeight };
  });
  // The steps stand in one column in walk order (the persisted walk/deep-link order). Only these walk links go
  // to dagre: given the flow's own edges, it routed every edge spanning several steps through dummy nodes it
  // placed in columns far beside the steps, drawing long diagonal "triangles" across the canvas. Edges are
  // routed below instead: straight between neighbours, through small side lanes otherwise.
  const ordered = [...graph.nodes].sort((a, b) => a.number - b.number);
  const rank = new Map(ordered.map((n, i) => [n.step_id, i]));
  const neighbours = (edge: GraphEdge) =>
    edge.from !== edge.to && Math.abs(rank.get(edge.from)! - rank.get(edge.to)!) === 1;
  for (let i = 1; i < ordered.length; i++) {
    const [v, w] = [ordered[i - 1]!.step_id, ordered[i]!.step_id];
    // The labels of the edges between these neighbours get room in the gap, beside the line.
    const labels = prepared.filter(
      (p) =>
        p.label !== null && neighbours(p.edge) && [v, w].includes(p.edge.from) && [v, w].includes(p.edge.to),
    );
    const size = {
      width: widest(labels.map((l) => l.size.width)),
      height: labels.reduce((n, l) => n + l.size.height + 2, 0),
    };
    g.setEdge(
      { v, w, name: `walk-${i}` },
      labels.length === 0
        ? { weight: 10000, minlen: 1 }
        : { weight: 10000, minlen: 1, ...size, labelpos: direction === "TB" ? "r" : "c", labeloffset: 6 },
    );
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

  return routeEdges(direction, laidOut, prepared, routed, rank, neighbours, (i) => {
    const e = g.edge({ v: ordered[i - 1]!.step_id, w: ordered[i]!.step_id, name: `walk-${i}` });
    return { x: e.x ?? 0, y: e.y ?? 0 };
  });
}

type Prepared = {
  edge: GraphEdge;
  key: string;
  lines: string[];
  size: { width: number; height: number };
  label: string | null;
};

/**
 * Routes the edges between the laid-out steps: neighbours in the column get one straight line (bottom centre to
 * top centre); every other edge leaves its step sideways into a lane a small fixed step beside the column —
 * forward edges on one side, backward edges on the other, overlapping spans in separate lanes — runs along it
 * and enters the target from the same side. Then the drawing is moved so everything starts at the margin.
 */
function routeEdges(
  direction: Direction,
  nodes: DiagramNode[],
  prepared: Prepared[],
  routed: Map<string, DiagramEdge>,
  rank: Map<StepId, number>,
  neighbours: (edge: GraphEdge) => boolean,
  walkLabel: (i: number) => Point,
): DiagramLayout {
  const tb = direction === "TB";
  // Main axis = along the column (y for TB), cross axis = across it.
  const at = (m: number, c: number): Point => (tb ? { x: c, y: m } : { x: m, y: c });
  const mOf = (b: Box) => (tb ? { start: b.y, size: b.height } : { start: b.x, size: b.width });
  const cOf = (b: Box) => (tb ? { start: b.x, size: b.width } : { start: b.y, size: b.height });
  const byId = new Map(nodes.map((n) => [n.stepId, n]));
  const loopReach = (n: DiagramNode) =>
    [...routed.values()]
      .filter((e) => e.from === n.stepId)
      .flatMap((e) => [
        ...e.points.map((p) => (tb ? p.x : p.y)),
        ...(e.labelBox ? [cOf(e.labelBox).start + cOf(e.labelBox).size] : []),
      ]);
  const cMax = widest(nodes.flatMap((n) => [cOf(n).start + cOf(n).size, ...loopReach(n)]));
  const cMin = Math.min(...nodes.map((n) => cOf(n).start));

  const edges = new Map<string, DiagramEdge>();
  const result = (p: Prepared, points: Point[], labelBox: Box | null): DiagramEdge => ({
    key: p.key,
    from: p.edge.from,
    to: p.edge.to,
    label: p.label,
    labelLines: p.lines,
    points,
    labelBox,
  });

  // Neighbours: one straight line; parallel ones side by side; labels in the gap the walk link reserved.
  const pairs = new Map<string, Prepared[]>();
  for (const p of prepared) {
    if (routed.has(p.key) || !neighbours(p.edge)) continue;
    const id = [p.edge.from, p.edge.to].sort().join(" ");
    pairs.set(id, [...(pairs.get(id) ?? []), p]);
  }
  for (const list of pairs.values()) {
    const first = list[0]!.edge;
    const upper = rank.get(first.from)! < rank.get(first.to)! ? first.from : first.to;
    const i = rank.get(upper)! + 1;
    const anchor = walkLabel(i);
    let labelAt = tb
      ? anchor.y - list.reduce((n, l) => n + (l.label === null ? 0 : l.size.height + 2), 0) / 2
      : 0;
    list.forEach((p, j) => {
      const from = byId.get(p.edge.from)!;
      const to = byId.get(p.edge.to)!;
      const shift = (j - (list.length - 1) / 2) * PARALLEL_STEP;
      const down = rank.get(p.edge.from)! < rank.get(p.edge.to)!;
      const side = (n: DiagramNode, end: boolean) => mOf(n).start + (end ? mOf(n).size : 0);
      const points = [
        at(side(from, down), cOf(from).start + cOf(from).size / 2 + shift),
        at(side(to, !down), cOf(to).start + cOf(to).size / 2 + shift),
      ];
      let labelBox: Box | null = null;
      if (p.label !== null) {
        labelBox = tb
          ? { x: anchor.x - widest(list.map((l) => l.size.width)) / 2, y: labelAt, ...p.size }
          : { x: anchor.x - p.size.width / 2, y: anchor.y - p.size.height / 2, ...p.size };
        labelAt += p.size.height + 2;
      }
      edges.set(p.key, result(p, points, labelBox));
    });
  }

  // Every other edge: a lane beside the column, forward on the far side, backward on the near side.
  const laned = prepared.filter((p) => !routed.has(p.key) && !edges.has(p.key) && p.edge.from !== p.edge.to);
  const span = (p: Prepared) => {
    const [a, b] = [mid(byId.get(p.edge.from)!), mid(byId.get(p.edge.to)!)];
    return [Math.min(a, b), Math.max(a, b)] as const;
  };
  const mid = (n: DiagramNode) => mOf(n).start + mOf(n).size / 2;
  const laneOf = new Map<string, number>();
  for (const forward of [true, false]) {
    const side = laned
      .filter((p) => rank.get(p.edge.from)! < rank.get(p.edge.to)! === forward)
      .sort((a, b) => span(a)[1] - span(a)[0] - (span(b)[1] - span(b)[0]));
    const lanes: (readonly [number, number])[][] = [];
    for (const p of side) {
      const [lo, hi] = span(p);
      let lane = lanes.findIndex((taken) => taken.every(([l, h]) => hi < l || lo > h));
      if (lane < 0) lane = lanes.push([]) - 1;
      lanes[lane]!.push([lo, hi]);
      laneOf.set(p.key, lane);
    }
  }
  // Ends on one side of a node are spread along it, ordered by where the other end is.
  const ends = new Map<string, { key: string; other: number }[]>();
  const sideKey = (id: StepId, forward: boolean) => `${id} ${forward}`;
  for (const p of laned) {
    const forward = rank.get(p.edge.from)! < rank.get(p.edge.to)!;
    for (const [id, other] of [
      [p.edge.from, p.edge.to],
      [p.edge.to, p.edge.from],
    ] as const) {
      const k = sideKey(id, forward);
      ends.set(k, [
        ...(ends.get(k) ?? []),
        { key: `${p.key}:${id === p.edge.from ? "s" : "e"}`, other: mid(byId.get(other)!) },
      ]);
    }
  }
  const attach = (id: StepId, forward: boolean, key: string) => {
    const list = [...ends.get(sideKey(id, forward))!].sort((a, b) => a.other - b.other);
    const n = byId.get(id)!;
    const i = list.findIndex((e) => e.key === key);
    const room = Math.max(0, mOf(n).size / 2 - 4);
    const offset = Math.max(-room, Math.min(room, (i - (list.length - 1) / 2) * ATTACH_STEP));
    return mid(n) + offset;
  };
  const placedLabels: Box[] = [];
  const free = (box: Box) =>
    ![
      ...nodes,
      ...placedLabels,
      ...[...edges.values()].flatMap((e) => (e.labelBox ? [e.labelBox] : [])),
    ].some(
      (o) =>
        box.x < o.x + o.width &&
        o.x < box.x + box.width &&
        box.y < o.y + o.height &&
        o.y < box.y + box.height,
    );
  for (const p of laned) {
    const forward = rank.get(p.edge.from)! < rank.get(p.edge.to)!;
    const offset = LANE_FIRST + LANE_STEP * laneOf.get(p.key)!;
    const laneC = forward ? cMax + offset : cMin - offset;
    const from = byId.get(p.edge.from)!;
    const to = byId.get(p.edge.to)!;
    const edgeC = (n: DiagramNode) => (forward ? cOf(n).start + cOf(n).size : cOf(n).start);
    const m1 = attach(p.edge.from, forward, `${p.key}:s`);
    const m2 = attach(p.edge.to, forward, `${p.key}:e`);
    const points = [at(m1, edgeC(from)), at(m1, laneC), at(m2, laneC), at(m2, edgeC(to))];
    let labelBox: Box | null = null;
    if (p.label !== null) {
      // On the lane, beside it on the outer side; moved along the lane when another label is there.
      const [lo, hi] = [Math.min(m1, m2), Math.max(m1, m2)];
      const along = tb ? p.size.height : p.size.width;
      const across = tb ? p.size.width : p.size.height;
      const c = forward ? laneC + 3 : laneC - 3 - across;
      const box = (m: number): Box => (tb ? { x: c, y: m, ...p.size } : { x: m, y: c, ...p.size });
      const centre = (lo + hi) / 2 - along / 2;
      const tries = [0, 1, -1, 2, -2, 3, -3, 4, -4].map((k) => centre + k * (along + 2));
      labelBox = box(tries.find((m) => free(box(m))) ?? centre);
      placedLabels.push(labelBox);
    }
    edges.set(p.key, result(p, points, labelBox));
  }

  // Move everything so the drawing starts at the margin.
  const all = prepared.map((p) => routed.get(p.key) ?? edges.get(p.key)!);
  const boxes: Box[] = [...nodes, ...all.flatMap((e) => (e.labelBox ? [e.labelBox] : []))];
  const points = all.flatMap((e) => e.points);
  const xs = [...boxes.flatMap((b) => [b.x, b.x + b.width]), ...points.map((p) => p.x)];
  const ys = [...boxes.flatMap((b) => [b.y, b.y + b.height]), ...points.map((p) => p.y)];
  const dx = MARGIN - Math.min(MARGIN, ...xs);
  const dy = MARGIN - Math.min(MARGIN, ...ys);
  const move = <T extends Point>(p: T): T => ({ ...p, x: p.x + dx, y: p.y + dy });
  return {
    direction,
    width: Math.max(0, ...xs) + dx + MARGIN,
    height: Math.max(0, ...ys) + dy + MARGIN,
    nodes: nodes.map(move),
    edges: all.map((e) => ({
      ...e,
      points: e.points.map(move),
      labelBox: e.labelBox && move(e.labelBox),
    })),
  };
}

/**
 * A stable top-to-bottom walk at every viewport width; numbering and deep links never depend on resizing.
 */
export function layoutForWidth(
  _width: number,
  _estimatedLeftToRight: number,
  get: (direction: Direction) => DiagramLayout,
): DiagramLayout {
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
