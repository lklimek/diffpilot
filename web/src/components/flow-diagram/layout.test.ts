import { describe, expect, it } from "vitest";
import type { FlowGraph, Tour } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { EDGE_LABEL_CHARS, NODE_LABEL_CHARS, layoutFlow, pickLayout, wrapText, type Box } from "./layout";
import { syntheticGraph } from "./syntheticGraph";

const tour = seedFor("default", "tour") as Tour;
const flow1 = tour.flows[0]!.graph;
const big = syntheticGraph(30);

const overlaps = (a: Box, b: Box) =>
  a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

describe("wrapText", () => {
  it("wraps on word boundaries without exceeding the line length", () => {
    const lines = wrapText("Wallet-derived candidates exist: format wallet sentence for the user", 22);
    expect(lines.join(" ")).toBe("Wallet-derived candidates exist: format wallet sentence for the user");
    for (const line of lines) expect(line.length).toBeLessThanOrEqual(22);
    expect(lines.length).toBeGreaterThan(1);
  });

  it("splits a word longer than the line and keeps every character", () => {
    const word = "RpcClient::send_with_retry_policy_and_backoff";
    const lines = wrapText(word, 20);
    expect(lines.join("")).toBe(word);
    for (const line of lines) expect(line.length).toBeLessThanOrEqual(20);
  });

  it("returns one empty line for empty text", () => {
    expect(wrapText("   ", 10)).toEqual([""]);
  });
});

describe("layoutFlow", () => {
  it.each(["TB", "LR"] as const)("places every node once with its full label (%s)", (direction) => {
    const layout = layoutFlow(flow1, direction);
    expect(layout.direction).toBe(direction);
    expect(layout.nodes.map((n) => n.stepId)).toEqual(flow1.nodes.map((n) => n.step_id));
    for (const [i, node] of layout.nodes.entries()) {
      expect(node.lines.join(" ")).toBe(flow1.nodes[i]!.label);
      for (const line of node.lines) expect(line.length).toBeLessThanOrEqual(NODE_LABEL_CHARS);
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
        for (const line of edge.labelLines) expect(line.length).toBeLessThanOrEqual(EDGE_LABEL_CHARS);
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
