import { describe, expect, it } from "vitest";
import type { FlowGraph, Tour } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import {
  EDGE_MAX_LINES,
  EDGE_TEXT_WIDTH,
  LABEL_MAX_CHARS,
  MAX_DRAWN_EDGES,
  MAX_DRAWN_STEPS,
  NODE_MAX_LINES,
  NODE_PADDING,
  NODE_TEXT_WIDTH,
  boundText,
  drawable,
  layoutFlow,
  pickLayout,
  wrapText,
  type Box,
  type DiagramLayout,
} from "./layout";
import { estimateWidth } from "./measure";
import { syntheticGraph } from "./syntheticGraph";

const tour = seedFor("default", "tour") as Tour;
const flow1 = tour.flows[0]!.graph;
const big = syntheticGraph(30);

const overlaps = (a: Box, b: Box) =>
  a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

const nodeWidth = (text: string) => estimateWidth(text, "node");

describe("wrapText", () => {
  it("wraps on word boundaries without exceeding the line width", () => {
    const text = "Wallet-derived candidates exist: format wallet sentence for the user";
    const lines = wrapText(text, 150, nodeWidth, 10);
    expect(lines.join(" ")).toBe(text);
    for (const line of lines) expect(nodeWidth(line)).toBeLessThanOrEqual(150);
    expect(lines.length).toBeGreaterThan(1);
  });

  it("splits a word wider than the line and keeps every character", () => {
    const word = "RpcClient::send_with_retry_policy_and_backoff";
    const lines = wrapText(word, 120, nodeWidth, 10);
    expect(lines.join("")).toBe(word);
    for (const line of lines) expect(nodeWidth(line)).toBeLessThanOrEqual(120);
  });

  it("wraps capitals and wide characters by their width, not their count", () => {
    for (const text of [
      "MAX_CONCURRENT_REQUESTS",
      "WWWWWWWWWWWWWWWWWWWW",
      "MMMM WWWW MMMM WWWW",
      "検証済みの再試行ポリシー",
    ]) {
      const lines = wrapText(text, 100, nodeWidth, 10);
      expect(lines.join("").replaceAll(" ", "")).toBe(text.replaceAll(" ", ""));
      for (const line of lines) expect(nodeWidth(line), line).toBeLessThanOrEqual(100);
    }
  });

  it("ends the last allowed line with a visible truncation mark", () => {
    const lines = wrapText("one two three four five six seven eight nine ten", 40, nodeWidth, 2);
    expect(lines).toHaveLength(2);
    expect(lines[1]!.endsWith("…")).toBe(true);
    for (const line of lines) expect(nodeWidth(line)).toBeLessThanOrEqual(40);
  });

  it("returns one empty line for empty text", () => {
    expect(wrapText("   ", 100, nodeWidth, 3)).toEqual([""]);
  });
});

describe("boundText", () => {
  it("keeps short text and cuts long text with a truncation mark", () => {
    expect(boundText("short", 10)).toBe("short");
    expect(boundText("x".repeat(20), 10)).toBe(`${"x".repeat(10)}…`);
  });

  it("never leaves half of a surrogate pair", () => {
    expect(boundText(`${"x".repeat(9)}😀tail`, 10)).toBe(`${"x".repeat(9)}…`);
  });
});

// Every point of every edge and every label inside the drawing.
function expectInside(layout: DiagramLayout) {
  for (const edge of layout.edges) {
    for (const p of edge.points) {
      expect(p.x, `${edge.key} x`).toBeGreaterThanOrEqual(0);
      expect(p.x, `${edge.key} x`).toBeLessThanOrEqual(layout.width);
      expect(p.y, `${edge.key} y`).toBeGreaterThanOrEqual(0);
      expect(p.y, `${edge.key} y`).toBeLessThanOrEqual(layout.height);
    }
    const box = edge.labelBox;
    if (box) {
      expect(box.x).toBeGreaterThanOrEqual(0);
      expect(box.y).toBeGreaterThanOrEqual(0);
      expect(box.x + box.width).toBeLessThanOrEqual(layout.width);
      expect(box.y + box.height).toBeLessThanOrEqual(layout.height);
    }
  }
}

const onBorder = (p: { x: number; y: number }, n: Box) => {
  const near = (a: number, b: number) => Math.abs(a - b) < 0.5;
  const inX = p.x >= n.x - 0.5 && p.x <= n.x + n.width + 0.5;
  const inY = p.y >= n.y - 0.5 && p.y <= n.y + n.height + 0.5;
  return (
    (inX && (near(p.y, n.y) || near(p.y, n.y + n.height))) ||
    (inY && (near(p.x, n.x) || near(p.x, n.x + n.width)))
  );
};

describe("layoutFlow", () => {
  it.each(["TB", "LR"] as const)("places every node once with its full label (%s)", (direction) => {
    const layout = layoutFlow(flow1, direction);
    expect(layout.direction).toBe(direction);
    expect(layout.nodes.map((n) => n.stepId)).toEqual(flow1.nodes.map((n) => n.step_id));
    for (const [i, node] of layout.nodes.entries()) {
      expect(node.lines.join(" ")).toBe(flow1.nodes[i]!.label);
      for (const line of node.lines) expect(nodeWidth(line)).toBeLessThanOrEqual(NODE_TEXT_WIDTH);
      expect(node.x).toBeGreaterThanOrEqual(0);
      expect(node.y).toBeGreaterThanOrEqual(0);
      expect(node.x + node.width).toBeLessThanOrEqual(layout.width);
      expect(node.y + node.height).toBeLessThanOrEqual(layout.height);
    }
  });

  it("routes every edge, including the loop back to step 2, with its branch label", () => {
    const layout = layoutFlow(flow1, "TB");
    expect(layout.edges).toHaveLength(flow1.edges.length);
    for (const [i, edge] of layout.edges.entries()) {
      const source = flow1.edges[i]!;
      expect([edge.from, edge.to, edge.label]).toEqual([source.from, source.to, source.label]);
      expect(edge.points.length).toBeGreaterThanOrEqual(2);
      expect(edge.labelBox === null).toBe(source.label === null);
      if (edge.label) {
        expect(edge.labelLines.join(" ")).toBe(edge.label);
        for (const line of edge.labelLines) {
          expect(estimateWidth(line, "edge")).toBeLessThanOrEqual(EDGE_TEXT_WIDTH);
        }
      }
    }
    const loop = layout.edges.find((e) => e.from === "f1.6" && e.to === "f1.2")!;
    expect(loop.label).toBe("loop");
  });

  it("marks steps with two or more outgoing edges as branching", () => {
    const layout = layoutFlow(flow1, "TB");
    const branching = layout.nodes.filter((n) => n.branching).map((n) => n.stepId);
    expect(branching).toEqual(["f1.3", "f1.5", "f1.6"]);
  });

  it("drops edges whose ends are not nodes of the graph", () => {
    const graph: FlowGraph = {
      nodes: flow1.nodes.slice(0, 2),
      edges: [
        { from: "f1.1", to: "f1.2", label: null },
        { from: "f1.2", to: "f9.9", label: "dangling" },
      ],
    };
    const layout = layoutFlow(graph, "TB");
    expect(layout.nodes.map((n) => n.stepId)).toEqual(["f1.1", "f1.2"]);
    expect(layout.edges.map((e) => e.label)).toEqual([null]);
    expect(layout.nodes.every((n) => !n.branching)).toBe(true);
  });

  it("keeps parallel edges between the same steps apart", () => {
    const graph: FlowGraph = {
      nodes: flow1.nodes.slice(0, 2),
      edges: [
        { from: "f1.1", to: "f1.2", label: "first" },
        { from: "f1.1", to: "f1.2", label: "second" },
      ],
    };
    const layout = layoutFlow(graph, "TB");
    expect(layout.edges.map((e) => e.label)).toEqual(["first", "second"]);
    expect(new Set(layout.edges.map((e) => e.key)).size).toBe(2);
  });

  it("sizes nodes and branch labels to the measured text, including capitals and long identifiers", () => {
    const graph: FlowGraph = {
      nodes: [
        { step_id: "a", number: 1, kind: "changed", label: "MAX_CONCURRENT_REQUESTS" },
        {
          step_id: "b",
          number: 12,
          kind: "added",
          label: "WWWW MMMM RpcClient::send_with_retry_policy_and_backoff",
        },
      ],
      edges: [{ from: "a", to: "b", label: "MAX_CONCURRENT_REQUESTS exceeded WWWW" }],
    };
    // A measure that makes capitals very wide: the layout must follow it, not a fixed width per character.
    const wideCaps = (text: string) => [...text].reduce((w, c) => w + (c >= "A" && c <= "Z" ? 11 : 6), 0);
    for (const direction of ["TB", "LR"] as const) {
      const layout = layoutFlow(graph, direction, wideCaps);
      for (const node of layout.nodes) {
        for (const line of node.lines) {
          expect(wideCaps(line), line).toBeLessThanOrEqual(NODE_TEXT_WIDTH);
          expect(node.x + NODE_PADDING.x + node.prefixWidth + wideCaps(line)).toBeLessThanOrEqual(
            node.x + node.width - NODE_PADDING.x,
          );
        }
        expect(node.prefixWidth).toBeGreaterThan(wideCaps(String(node.number)));
      }
      const label = layout.edges[0]!;
      for (const line of label.labelLines) expect(wideCaps(line)).toBeLessThan(label.labelBox!.width);
      expectInside(layout);
    }
  });

  it("bounds a multi-megabyte label to a few lines ending in a truncation mark", () => {
    const graph: FlowGraph = {
      nodes: [{ step_id: "a", number: 1, kind: "added", label: "x".repeat(3_200_000) }],
      edges: [{ from: "a", to: "a", label: "y ".repeat(1_600_000) }],
    };
    for (const direction of ["TB", "LR"] as const) {
      const layout = layoutFlow(graph, direction);
      const [node] = layout.nodes;
      expect(node!.lines.length).toBeLessThanOrEqual(NODE_MAX_LINES);
      expect(node!.lines.join("").length).toBeLessThanOrEqual(LABEL_MAX_CHARS + 1);
      expect(node!.lines.at(-1)!.endsWith("…")).toBe(true);
      expect(layout.edges[0]!.labelLines.length).toBeLessThanOrEqual(EDGE_MAX_LINES);
      expect(layout.edges[0]!.labelLines.at(-1)!.endsWith("…")).toBe(true);
    }
  });

  it.each(["TB", "LR"] as const)(
    "routes a self-loop from the node back to it, inside the drawing (%s)",
    (direction) => {
      const graph: FlowGraph = {
        nodes: [
          { step_id: "a", number: 1, kind: "changed", label: "Send the request" },
          { step_id: "b", number: 2, kind: "added", label: "Repeat until done" },
          { step_id: "c", number: 3, kind: "changed", label: "Return the response" },
        ],
        edges: [
          { from: "a", to: "b", label: null },
          { from: "b", to: "b", label: "retry" },
          { from: "b", to: "b", label: null },
          { from: "b", to: "c", label: "done" },
        ],
      };
      const layout = layoutFlow(graph, direction);
      const node = layout.nodes.find((n) => n.stepId === "b")!;
      const loops = layout.edges.filter((e) => e.from === "b" && e.to === "b");
      expect(loops).toHaveLength(2);
      for (const loop of loops) {
        expect(loop.points.length).toBeGreaterThanOrEqual(3);
        expect(onBorder(loop.points[0]!, node), "start on the node").toBe(true);
        expect(onBorder(loop.points.at(-1)!, node), "end on the node").toBe(true);
      }
      for (const edge of layout.edges.filter((e) => e.from !== e.to)) {
        const from = layout.nodes.find((n) => n.stepId === edge.from)!;
        const to = layout.nodes.find((n) => n.stepId === edge.to)!;
        expect(onBorder(edge.points[0]!, from), `${edge.key} starts on its node`).toBe(true);
        expect(onBorder(edge.points.at(-1)!, to), `${edge.key} ends on its node`).toBe(true);
      }
      expectInside(layout);
      const boxes: Box[] = [
        ...layout.nodes,
        ...layout.edges.flatMap((e) => (e.labelBox ? [e.labelBox] : [])),
      ];
      for (let i = 0; i < boxes.length; i += 1) {
        for (let j = i + 1; j < boxes.length; j += 1) expect(overlaps(boxes[i]!, boxes[j]!)).toBe(false);
      }
    },
  );

  it("keeps edges of fixture flow 1 inside the drawing", () => {
    expectInside(layoutFlow(flow1, "TB"));
    expectInside(layoutFlow(flow1, "LR"));
  });

  it("lays out an empty graph as an empty drawing", () => {
    expect(layoutFlow({ nodes: [], edges: [] }, "TB")).toMatchObject({ nodes: [], edges: [] });
  });

  it.each([
    ["fixture flow 1", flow1],
    ["synthetic 30-step flow", big],
  ])("never overlaps nodes or branch labels (%s)", (_, graph) => {
    for (const direction of ["TB", "LR"] as const) {
      const layout = layoutFlow(graph, direction);
      const boxes: Box[] = [
        ...layout.nodes,
        ...layout.edges.flatMap((e) => (e.labelBox ? [e.labelBox] : [])),
      ];
      for (let i = 0; i < boxes.length; i += 1) {
        for (let j = i + 1; j < boxes.length; j += 1) {
          expect(overlaps(boxes[i]!, boxes[j]!), `${direction} boxes ${i} and ${j}`).toBe(false);
        }
      }
    }
  });
});

describe("drawable", () => {
  it("draws flows up to the step and edge caps and not beyond", () => {
    expect(drawable(syntheticGraph(MAX_DRAWN_STEPS))).toBe(true);
    expect(drawable(syntheticGraph(MAX_DRAWN_STEPS + 1))).toBe(false);
    const nodes = syntheticGraph(2).nodes;
    const edges = Array.from({ length: MAX_DRAWN_EDGES + 1 }, () => ({
      from: "s1.1",
      to: "s1.2",
      label: null,
    }));
    expect(drawable({ nodes, edges })).toBe(false);
    expect(drawable({ nodes, edges: edges.slice(1) })).toBe(true);
  });

  it("lays out the largest drawable flow in reasonable time", () => {
    const start = performance.now();
    layoutFlow(syntheticGraph(MAX_DRAWN_STEPS), "TB");
    layoutFlow(syntheticGraph(MAX_DRAWN_STEPS), "LR");
    expect(performance.now() - start).toBeLessThan(2000);
  });
});

describe("pickLayout", () => {
  const layouts = { TB: layoutFlow(flow1, "TB"), LR: layoutFlow(flow1, "LR") };

  it("uses the wide left-to-right drawing when it fits the available width", () => {
    expect(pickLayout(layouts, layouts.LR.width).direction).toBe("LR");
  });

  it("falls back to top-to-bottom when left-to-right does not fit or the width is unknown", () => {
    expect(pickLayout(layouts, layouts.LR.width - 1).direction).toBe("TB");
    expect(pickLayout(layouts, 0).direction).toBe("TB");
  });

  // Centre column content per mockup 05-tour: 1440 − nav 270 − inspector 389 − gutters 12 − padding 28 = 741;
  // at 3440: 3440 − 400 − 620 − 12 − 28 = 2380.
  it("draws fixture flow 1 left to right at 3440 px and top to bottom, unscrolled sideways, at 1440", () => {
    expect(pickLayout(layouts, 2380).direction).toBe("LR");
    expect(pickLayout(layouts, 741).direction).toBe("TB");
    expect(layouts.TB.width).toBeLessThanOrEqual(741);
  });

  it("draws a 30-step flow top to bottom within the 1440 px centre column", () => {
    const big30 = { TB: layoutFlow(big, "TB"), LR: layoutFlow(big, "LR") };
    expect(pickLayout(big30, 2380).direction).toBe("TB");
    expect(big30.TB.width).toBeLessThanOrEqual(741);
  });
});
