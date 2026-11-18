import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "react";
import { describe, expect, it, vi } from "vitest";
import type { PendingReview, ReviewState, Tour } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { FIXTURE_STATES, seedFor } from "../../mocks/fixtures";
import { NavList } from "./NavList";

const tour = seedFor("default", "tour") as Tour;

function renderNav(props: Partial<ComponentProps<typeof NavList>> = {}) {
  return render(
    <NavList
      flows={tour.flows}
      quiet={tour.quiet}
      ticked={[]}
      pendingCount={0}
      current={{ block: "pr_context" }}
      onSelect={() => {}}
      {...props}
    />,
  );
}

describe("NavList", () => {
  it("lists PR context and the flows in tour order; marks the PR context as current", () => {
    renderNav();
    expect(screen.getByRole("button", { name: "PR context" })).toHaveAttribute("aria-current", "step");
    const flows = screen.getAllByRole("button", { name: /^Flow \d/ });
    expect(flows.map((b) => b.textContent)).toEqual(
      tour.flows.map((f) => expect.stringContaining(f.llm.title)),
    );
    // No flow is open: steps are listed only for the current flow.
    expect(screen.queryByRole("button", { name: /^Step / })).not.toBeInTheDocument();
  });

  it("draws the PR context row as a menu row like the flow rows, not as a heading", () => {
    renderNav({ current: { block: "flow_intro", flow_id: "f1" } });
    const row = screen.getByRole("button", { name: "PR context" });
    const flowRow = screen.getByRole("button", { name: /^Flow 1/ });
    expect(row).toHaveAttribute("data-row", "menu");
    expect(flowRow).toHaveAttribute("data-row", "menu");
    expect(row).toHaveAccessibleDescription("Problem, related work, fit questions, CI");
  });

  it("opens the current flow's numbered steps and marks the current step", () => {
    renderNav({ current: { block: "step", step_id: "f1.5" } });
    const steps = screen.getAllByRole("button", { name: /^Step / });
    expect(steps).toHaveLength(10);
    expect(steps[4]).toHaveAccessibleName("Step 5: Retry allowed?");
    expect(steps[4]).toHaveAttribute("aria-current", "step");
    expect(screen.getByRole("button", { name: /^Flow 1/ })).not.toHaveAttribute("aria-current");
  });

  it("shows each flow's LLM risk label and selects blocks through the callback", async () => {
    const onSelect = vi.fn();
    renderNav({ current: { block: "flow_intro", flow_id: "f1" }, onSelect });
    const flow1 = screen.getByRole("button", { name: /^Flow 1/ });
    expect(flow1).toHaveAttribute("aria-current", "step");
    expect(within(flow1.closest("li")!).getByText("▲ HIGH")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /^Flow 2/ }));
    await userEvent.click(
      screen.getByRole("button", { name: "Step 2: Attempt: build the request and send once" }),
    );
    await userEvent.click(screen.getByRole("button", { name: "PR context" }));
    expect(onSelect.mock.calls.map((c) => c[0])).toEqual([
      { block: "flow_intro", flow_id: "f2" },
      { block: "step", step_id: "f1.2" },
      { block: "pr_context" },
    ]);
  });

  it("labels LLM-classified refactor and test flows", () => {
    const flows = structuredClone(tour.flows);
    flows[3]!.group = "llm_classified_refactor";
    flows[4]!.group = "llm_classified_tests";
    renderNav({ flows });
    const row = (n: number) => screen.getByRole("button", { name: new RegExp(`^Flow ${n}`) }).closest("li")!;
    expect(within(row(4)).getByText("LLM-classified refactor")).toBeInTheDocument();
    expect(within(row(5)).getByText("LLM-classified tests")).toBeInTheDocument();
    expect(within(row(1)).queryByText(/LLM-classified/)).not.toBeInTheDocument();
  });

  it("lists the quiet rows after every flow, Unexplained first, with deterministic counters", async () => {
    const onSelect = vi.fn();
    renderNav({ onSelect });
    const rows = [
      ...within(screen.getByRole("list", { name: "Quiet rows" })).getAllByRole("button"),
      ...within(screen.getByRole("list", { name: "Not analysed" })).getAllByRole("button"),
    ];
    expect(rows.map((r) => r.textContent)).toEqual([
      "Unexplained — read these yourself6 changed lines in 1 file",
      "Moves with identical text1 moved block",
      "Tests · 4 files",
      "Docs & comments · 3 files",
      "Mechanical & generated · 1 file",
    ]);
    const lastFlow = screen.getAllByRole("button", { name: /^Flow \d/ }).at(-1)!;
    expect(lastFlow.compareDocumentPosition(rows[0]!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    for (const row of rows) await userEvent.click(row);
    expect(onSelect.mock.calls.map((c) => c[0])).toEqual([
      { block: "quiet", row: { row: "unexplained" } },
      { block: "quiet", row: { row: "moves" } },
      { block: "quiet", row: { row: "not_analysed", category: "tests" } },
      { block: "quiet", row: { row: "not_analysed", category: "docs_comments" } },
      { block: "quiet", row: { row: "not_analysed", category: "generated_mechanical" } },
    ]);
  });

  it("counts an unexplained file with base and head entries once", () => {
    renderNav({
      quiet: {
        ...tour.quiet,
        unexplained: [
          { file: "src/a.rs", side: "base", ranges: [{ start_line: 3, end_line: 3 }] },
          { file: "src/a.rs", side: "head", ranges: [{ start_line: 4, end_line: 4 }] },
        ],
      },
    });
    expect(screen.getByRole("button", { name: /^Unexplained/ })).toHaveTextContent(
      "2 changed lines in 1 file",
    );
  });

  it("omits quiet rows that have no material", () => {
    renderNav({ quiet: { unexplained: [], moves: [], not_analysed: [] } });
    expect(screen.queryByRole("list", { name: "Quiet rows" })).not.toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Not analysed" })).not.toBeInTheDocument();
    expect(screen.queryByText(/Unexplained/)).not.toBeInTheDocument();
  });

  it("marks the current quiet row", () => {
    renderNav({ current: { block: "quiet", row: { row: "not_analysed", category: "tests" } } });
    expect(screen.getByRole("button", { name: "Tests · 4 files" })).toHaveAttribute("aria-current", "true");
    expect(screen.getByRole("button", { name: /^Docs/ })).not.toHaveAttribute("aria-current");
  });

  it("shows the pending-review row with its count and opens the list", async () => {
    const onSelect = vi.fn();
    renderNav({ pendingCount: 2, onSelect });
    const row = screen.getByRole("button", { name: "Pending review on GitHub: 2 comments" });
    await userEvent.click(row);
    expect(onSelect).toHaveBeenCalledWith({ block: "pending_review" });
  });

  it("marks an incomplete pending-review count", () => {
    renderNav({ pendingCount: "2+" });
    expect(screen.getByRole("button", { name: "Pending review on GitHub: 2+ comments" })).toBeInTheDocument();
  });

  it("says when commenting is unavailable and while the count is loading", () => {
    const { unmount } = renderNav({ pendingCount: "unavailable" });
    expect(screen.getByRole("button", { name: /^Pending review on GitHub/ })).toHaveTextContent(
      "not available",
    );
    unmount();
    renderNav({ pendingCount: "loading" });
    expect(screen.getByRole("button", { name: /^Pending review on GitHub/ })).toHaveTextContent("loading");
  });

  it("has no pending-review row when no count is given", () => {
    renderNav({ pendingCount: undefined });
    expect(screen.queryByRole("button", { name: /^Pending review/ })).not.toBeInTheDocument();
  });

  it("ticks reviewed steps and flows whose steps are all ticked", () => {
    renderNav({ ticked: ["f2.1", "f2.2", "f1.1"], current: { block: "step", step_id: "f1.1" } });
    expect(
      screen.getByRole("button", { name: "Flow 2: Default timeout 30 s → 5 s, reviewed" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Flow 1: Send an RPC request with retry" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Step 1: Trigger: callers of send, reviewed" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Step 2: Attempt: build the request and send once" }),
    ).toBeInTheDocument();
  });

  it("shows the keyboard hints for blocks, flows and help", () => {
    renderNav();
    expect(screen.getByText(/next \/ previous block/)).toHaveTextContent("n p flow · ? help");
  });

  it.each(FIXTURE_STATES)("renders the menu and keeps the wording rules in state %s", (state) => {
    const stateTour = seedFor(state, "tour") as Tour;
    const review = seedFor(state, "review-state") as ReviewState;
    const pending = seedFor(state, "pending-review") as PendingReview;
    const { container } = renderNav({
      flows: stateTour.flows,
      quiet: stateTour.quiet,
      ticked: review.ticked_steps,
      pendingCount: pending.comments.length,
      current: review.last_viewed ?? { block: "pr_context" },
    });
    expect(screen.getAllByRole("button", { name: /^Flow \d/ })).toHaveLength(stateTour.flows.length);
    expect(screen.getByRole("button", { name: /^Pending review on GitHub/ })).toHaveTextContent(
      String(pending.comments.length),
    );
    expectTrustWording(container.textContent ?? "");
  });
});

describe("NavList: the user's questions", () => {
  it("marks steps with the user's questions: count and tooltip", () => {
    renderNav({ current: { block: "step", step_id: "f1.5" }, questionCounts: { "f1.5": 2, "f1.2": 1 } });
    const marker = screen.getByText("?2");
    expect(marker).toHaveAccessibleDescription("2 questions asked on this step");
    expect(marker).toHaveAttribute("tabindex", "0");
    expect(screen.getByText("?1")).toHaveAccessibleDescription("1 question asked on this step");
    expect(screen.getByRole("button", { name: "Step 5: Retry allowed?, 2 questions asked" })).toBeVisible();
    expect(screen.getAllByText(/^\?\d$/)).toHaveLength(2);
  });

  it("one entry with the count opens the questions page; while open, it is the current row", async () => {
    const onOpen = vi.fn();
    const { rerender } = renderNav({ questions: { count: 3, open: false, onOpen } });
    const row = screen.getByRole("button", { name: "Your questions and answers: 3" });
    expect(row).toHaveAccessibleDescription(/only you see them/i);
    await userEvent.click(row);
    expect(onOpen).toHaveBeenCalledOnce();
    rerender(
      <NavList
        flows={tour.flows}
        quiet={tour.quiet}
        ticked={[]}
        current={{ block: "pr_context" }}
        onSelect={() => {}}
        questions={{ count: 3, open: true, onOpen }}
      />,
    );
    expect(screen.getByRole("button", { name: "Your questions and answers: 3" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(screen.getByRole("button", { name: "PR context" })).not.toHaveAttribute("aria-current");
  });
});
