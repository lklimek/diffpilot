import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { FlowGraph, Tour } from "../../api/generated/types";
import { expectTrustWording, toursByState } from "../../__tests__/wording";
import { seedFor } from "../../mocks/fixtures";
import { FlowDiagram } from "./FlowDiagram";
import { LABEL_MAX_CHARS, MAX_LISTED_EDGES, MAX_LISTED_STEPS, TEXT_MAX_CHARS } from "./layout";
import { syntheticGraph } from "./syntheticGraph";

const tour = seedFor("default", "tour") as Tour;
const flow1 = tour.flows[0]!.graph;

function renderDiagram(graph: FlowGraph = flow1, currentStepId: string | null = "f1.3") {
  const onSelectStep = vi.fn();
  const view = render(
    <FlowDiagram
      graph={graph}
      title="Retry on timeout"
      currentStepId={currentStepId}
      onSelectStep={onSelectStep}
    />,
  );
  return { ...view, onSelectStep };
}

const diagram = () => screen.getByRole("group", { name: /^Flow diagram/ });
const nodes = () => within(diagram()).getAllByRole("button");

describe("FlowDiagram structure", () => {
  it("draws one numbered node per step with its label and names the diagram", () => {
    renderDiagram();
    expect(diagram()).toHaveAccessibleName("Flow diagram: Retry on timeout, 10 steps");
    expect(nodes()).toHaveLength(10);
    for (const [i, node] of flow1.nodes.entries()) {
      const drawn = nodes()[i]!;
      expect(drawn).toHaveAttribute("data-step", node.step_id);
      expect(drawn).toHaveTextContent(String(node.number));
      expect(drawn.textContent?.replace(/\s+/g, " ")).toContain(node.label);
    }
  });

  it("names each node by number, kind, branching and label", () => {
    renderDiagram();
    expect(
      screen.getByRole("button", { name: "Step 1, unchanged context: Trigger: callers of send" }),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "Step 3, changed, branching: Outcome of the attempt?" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Step 7, added: Give up: Exhausted" })).toBeInTheDocument();
  });

  it("draws every branch label", () => {
    renderDiagram();
    const labels = [...diagram().querySelectorAll("[data-edge-label]")].map((l) =>
      l.textContent?.replace(/\s+/g, " ").trim(),
    );
    const expected = flow1.edges.flatMap((e) => (e.label ? [e.label] : []));
    expect(labels.sort()).toEqual(expected.sort());
    expect(diagram().querySelectorAll("[data-edge]")).toHaveLength(flow1.edges.length);
  });

  it("encodes the kind by colour AND by border style and marker glyph", () => {
    renderDiagram();
    const look = (stepId: string) => {
      const node = diagram().querySelector(`[data-step="${stepId}"]`)!;
      const shape = node.querySelector("[data-shape]")!;
      return {
        kind: node.getAttribute("data-kind"),
        colour: shape.getAttribute("class"),
        dash: shape.getAttribute("stroke-dasharray"),
        glyph: node.querySelector("[data-glyph]")!.textContent,
      };
    };
    const added = look("f1.5");
    const changed = look("f1.2");
    const unchanged = look("f1.1");
    expect([added.kind, changed.kind, unchanged.kind]).toEqual(["added", "changed", "unchanged"]);
    expect([added.glyph, changed.glyph, unchanged.glyph]).toEqual(["+", "~", "="]);
    expect(new Set([added.dash, changed.dash, unchanged.dash]).size).toBe(3);
    expect(new Set([added.colour, changed.colour, unchanged.colour]).size).toBe(3);
  });

  it("draws branching steps with pointed ends and others as rectangles", () => {
    renderDiagram();
    const shape = (stepId: string) =>
      diagram().querySelector(`[data-step="${stepId}"] [data-shape]`)!.tagName.toLowerCase();
    expect(shape("f1.3")).toBe("polygon");
    expect(shape("f1.4")).toBe("rect");
  });

  it("shows a legend for kinds, branching steps and the current step", () => {
    renderDiagram();
    const legend = screen.getByRole("list", { name: "Diagram legend" });
    for (const entry of ["+ added", "~ changed", "= unchanged context", "branching step", "current step"]) {
      expect(legend).toHaveTextContent(entry);
    }
    expect(legend).toHaveTextContent("Step kinds are LLM analysis.");
  });

  it("says when a flow has no steps to draw", () => {
    renderDiagram({ nodes: [], edges: [] }, null);
    expect(screen.getByText("This flow has no steps to draw.")).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: /^Flow diagram/ })).not.toBeInTheDocument();
  });
});

describe("FlowDiagram width", () => {
  // Reports a fixed content width to every observed element, like a 3440 px centre column.
  class WideObserver {
    constructor(private readonly callback: ResizeObserverCallback) {}
    observe() {
      this.callback(
        [{ contentRect: { width: 2380 } } as ResizeObserverEntry],
        this as unknown as ResizeObserver,
      );
    }
    disconnect() {}
  }

  it("draws top to bottom while the width is unknown", () => {
    renderDiagram();
    expect(diagram()).toHaveAttribute("data-direction", "TB");
  });

  it("uses a wide column for the left-to-right drawing, also when the steps arrive later", () => {
    vi.stubGlobal("ResizeObserver", WideObserver);
    try {
      const { rerender, onSelectStep } = renderDiagram({ nodes: [], edges: [] }, null);
      rerender(
        <FlowDiagram
          graph={flow1}
          title="Retry on timeout"
          currentStepId={null}
          onSelectStep={onSelectStep}
        />,
      );
      expect(diagram()).toHaveAttribute("data-direction", "TB");
    } finally {
      vi.unstubAllGlobals();
    }
  });
});

describe("FlowDiagram current step and selection", () => {
  it("marks only the current step", () => {
    renderDiagram(flow1, "f1.5");
    const current = nodes().filter((n) => n.getAttribute("aria-current") === "step");
    expect(current.map((n) => n.getAttribute("data-step"))).toEqual(["f1.5"]);
    expect(current[0]!.querySelector("[data-current-outline]")).not.toBeNull();
    expect(diagram().querySelectorAll("[data-current-outline]")).toHaveLength(1);
  });

  it("marks nothing when no step is current", () => {
    renderDiagram(flow1, null);
    expect(nodes().filter((n) => n.hasAttribute("aria-current"))).toHaveLength(0);
  });

  it("calls onSelectStep with the clicked node's step", async () => {
    const { onSelectStep } = renderDiagram();
    await userEvent.click(screen.getByRole("button", { name: /^Step 6,/ }));
    expect(onSelectStep).toHaveBeenCalledExactlyOnceWith("f1.6");
  });

  it("is one tab stop on the current step; arrows, Home and End move; Enter and Space select", async () => {
    const user = userEvent.setup();
    const { onSelectStep } = renderDiagram(flow1, "f1.3");
    expect(
      nodes()
        .filter((n) => n.tabIndex === 0)
        .map((n) => n.dataset.step),
    ).toEqual(["f1.3"]);
    await user.tab();
    expect(document.activeElement).toHaveAttribute("data-step", "f1.3");
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement).toHaveAttribute("data-step", "f1.4");
    await user.keyboard("{ArrowRight}");
    expect(document.activeElement).toHaveAttribute("data-step", "f1.5");
    await user.keyboard("{ArrowUp}{ArrowLeft}");
    expect(document.activeElement).toHaveAttribute("data-step", "f1.3");
    await user.keyboard("{End}");
    expect(document.activeElement).toHaveAttribute("data-step", "f1.10");
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement).toHaveAttribute("data-step", "f1.10");
    await user.keyboard("{Enter}");
    expect(onSelectStep).toHaveBeenLastCalledWith("f1.10");
    await user.keyboard("{Home}");
    expect(document.activeElement).toHaveAttribute("data-step", "f1.1");
    await user.keyboard(" ");
    expect(onSelectStep).toHaveBeenLastCalledWith("f1.1");
    expect(onSelectStep).toHaveBeenCalledTimes(2);
    expect(
      nodes()
        .filter((n) => n.tabIndex === 0)
        .map((n) => n.dataset.step),
    ).toEqual(["f1.1"]);
  });

  it("moves the tab stop to a new current step", () => {
    const { rerender, onSelectStep } = renderDiagram(flow1, "f1.3");
    rerender(
      <FlowDiagram graph={flow1} title="Retry on timeout" currentStepId="f1.8" onSelectStep={onSelectStep} />,
    );
    expect(
      nodes()
        .filter((n) => n.tabIndex === 0)
        .map((n) => n.dataset.step),
    ).toEqual(["f1.8"]);
  });

  it("puts the first step in the tab order when no step is current", () => {
    renderDiagram(flow1, null);
    expect(
      nodes()
        .filter((n) => n.tabIndex === 0)
        .map((n) => n.dataset.step),
    ).toEqual(["f1.1"]);
  });
});

describe("FlowDiagram text alternative", () => {
  it("lists every step in order with its kind and outgoing edges with branch labels", () => {
    renderDiagram();
    const list = screen.getByRole("list", { name: "Flow steps and edges" });
    const items = within(list)
      .getAllByRole("listitem")
      .filter((li) => li.parentElement === list);
    expect(items).toHaveLength(10);
    const text = (i: number) => items[i]!.textContent?.replace(/\s+/g, " ").trim();
    expect(text(0)).toBe("Step 1, unchanged context: Trigger: callers of send — goes to step 2");
    expect(text(2)).toBe(
      "Step 3, changed, branching: Outcome of the attempt? — goes to step 4 (Ok), step 5 (Timeout), step 8 (other error)",
    );
    expect(text(6)).toBe("Step 7, added: Give up: Exhausted — no outgoing edge");
  });

  it("is a collapsed disclosure the user can open", async () => {
    renderDiagram();
    const summary = screen.getByText("Diagram as text");
    expect(summary.closest("details")).not.toHaveAttribute("open");
    await userEvent.click(summary);
    expect(summary.closest("details")).toHaveAttribute("open");
  });
});

describe("FlowDiagram limits", () => {
  it("draws a multi-megabyte label as a few lines with a truncation mark and bounds its accessible name", () => {
    const label = "x".repeat(3_200_000);
    renderDiagram({ nodes: [{ step_id: "a", number: 1, kind: "added", label }], edges: [] }, "a");
    const node = nodes()[0]!;
    expect(node.textContent).toContain("…");
    expect(node.textContent!.length).toBeLessThan(LABEL_MAX_CHARS + 20);
    expect(node.getAttribute("aria-label")!.length).toBeLessThan(TEXT_MAX_CHARS + 40);
    const item = within(screen.getByRole("list", { name: "Flow steps and edges" })).getByRole("listitem");
    expect(item.textContent).toContain("…");
    expect(item.textContent!.length).toBeLessThan(TEXT_MAX_CHARS + 60);
  });

  it("shows a flow with thousands of steps and edges as text with a plain note instead of drawing it", () => {
    const graph = syntheticGraph(3000);
    graph.edges.push(...graph.edges.map((e) => ({ ...e, label: null })));
    const start = performance.now();
    renderDiagram(graph, "s1.17");
    expect(performance.now() - start).toBeLessThan(3000);
    expect(screen.queryByRole("group", { name: /^Flow diagram/ })).not.toBeInTheDocument();
    expect(
      screen.getByText(
        `This flow has 3000 steps and ${graph.edges.length} edges: too many to draw. The list below shows it as text.`,
      ),
    ).toBeInTheDocument();
    const list = screen.getByRole("list", { name: "Flow steps and edges" });
    expect(list.closest("details")).toHaveAttribute("open");
    expect(within(list).getAllByRole("listitem")).toHaveLength(MAX_LISTED_STEPS);
    expect(screen.getByText(`${3000 - MAX_LISTED_STEPS} more steps are not listed.`)).toBeInTheDocument();
  });

  it("lists at most a bounded number of outgoing edges per step", () => {
    const graph = syntheticGraph(MAX_LISTED_EDGES + 5);
    graph.edges = graph.nodes.slice(1).map((n) => ({ from: "s1.1", to: n.step_id, label: null }));
    renderDiagram(graph, null);
    const first = within(screen.getByRole("list", { name: "Flow steps and edges" })).getAllByRole(
      "listitem",
    )[0]!;
    expect(first.textContent).toContain(`and ${MAX_LISTED_EDGES + 4 - MAX_LISTED_EDGES} more`);
  });

  it("draws a self-loop with its branch label", () => {
    renderDiagram(
      {
        nodes: [{ step_id: "a", number: 1, kind: "changed", label: "Repeat until done" }],
        edges: [{ from: "a", to: "a", label: "retry" }],
      },
      "a",
    );
    expect(diagram().querySelectorAll("[data-edge]")).toHaveLength(1);
    expect(diagram().querySelector("[data-edge-label]")).toHaveTextContent("retry");
    expect(screen.getByRole("list", { name: "Flow steps and edges" })).toHaveTextContent(
      "Step 1, changed: Repeat until done — goes to step 1 (retry)",
    );
  });
});

describe("FlowDiagram text measurement", () => {
  // Counts canvas measurements per text so a released cache shows up as a new measurement.
  function stubCanvas() {
    const measured = new Map<string, number>();
    vi.stubGlobal(
      "OffscreenCanvas",
      class {
        getContext() {
          return {
            font: "",
            measureText: (text: string) => {
              measured.set(text, (measured.get(text) ?? 0) + 1);
              return { width: text.length * 7 };
            },
          };
        }
      },
    );
    return measured;
  }
  const one = (label: string): FlowGraph => ({
    nodes: [{ step_id: "a", number: 1, kind: "added", label }],
    edges: [],
  });

  it("keeps measurements while the graph stays and releases them when the graph changes", () => {
    const measured = stubCanvas();
    try {
      const first = one("First label");
      const { rerender, onSelectStep } = renderDiagram(first, null);
      const props = { title: "t", onSelectStep };
      const measuredFirst = measured.get("First label");
      expect(measuredFirst).toBeGreaterThan(0);
      rerender(<FlowDiagram graph={first} currentStepId="a" {...props} />);
      expect(measured.get("First label")).toBe(measuredFirst);
      rerender(<FlowDiagram graph={one("Second label")} currentStepId={null} {...props} />);
      rerender(<FlowDiagram graph={first} currentStepId={null} {...props} />);
      expect(measured.get("First label")).toBeGreaterThan(measuredFirst!);
    } finally {
      vi.unstubAllGlobals();
    }
  });
});

describe("FlowDiagram wording", () => {
  it.each(toursByState)("rendered text follows the trust rules (%s)", (_, stateTour) => {
    for (const flow of stateTour.flows) {
      const { container, unmount } = render(
        <FlowDiagram
          graph={flow.graph}
          title={flow.llm.title}
          currentStepId={null}
          onSelectStep={() => {}}
        />,
      );
      expectTrustWording(container.textContent ?? "");
      expect(container.querySelector("a")).toBeNull();
      unmount();
    }
  });

  it("rendered text of a 30-step flow follows the trust rules", () => {
    const { container } = renderDiagram(syntheticGraph(30), "s1.17");
    expect(nodes()).toHaveLength(30);
    expectTrustWording(container.textContent ?? "");
  });
});
