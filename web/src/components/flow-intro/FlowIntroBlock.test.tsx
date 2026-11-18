import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ReviewState, Tour } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { FIXTURE_STATES, seedFor } from "../../mocks/fixtures";
import { FlowIntroBlock } from "./FlowIntroBlock";

const tour = seedFor("default", "tour") as Tour;
const [f1, f2, f3, , f5] = tour.flows as [Tour["flows"][number], ...Tour["flows"]];

describe("FlowIntroBlock", () => {
  it("shows title, LLM risk, the three introduction fields and the LLM tag", () => {
    render(<FlowIntroBlock flow={f1} dependencies={[f3!, f5!]} ticked={[]} />);
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("1 Send an RPC request with retry");
    const llm = screen.getByRole("region", { name: "LLM analysis" });
    expect(within(llm).getByText("LLM")).toHaveAccessibleDescription(/written by the LLM/);
    expect(llm).not.toHaveTextContent(/unless marked otherwise/);
    expect(within(llm).getByText("Problem (before this PR)")).toBeInTheDocument();
    expect(within(llm).getByText("Change (what this PR does)")).toBeInTheDocument();
    expect(within(llm).getByText("Before / After example")).toBeInTheDocument();
    expect(llm).toHaveTextContent(f1.llm.introduction.example_before_after);
    expect(llm).toHaveTextContent("Flow 3: New error variant RpcError::Exhausted");
    expect(screen.getByText("▲ HIGH")).toBeInTheDocument();
    expect(screen.queryByText("Was wrong")).not.toBeInTheDocument();
  });

  it("lists the steps and files of the LLM's split under the LLM analysis, not under a no-LLM subtitle", () => {
    render(<FlowIntroBlock flow={f1} dependencies={[]} ticked={[]} />);
    expect(screen.queryByText(/no LLM/)).not.toBeInTheDocument();
    const llm = screen.getByRole("region", { name: "LLM analysis" });
    const dd = within(llm).getByText("Steps").nextElementSibling!;
    expect(dd).toHaveTextContent("10 steps in");
    for (const file of f1.facts.files) expect(dd).toHaveTextContent(file);
  });

  it("shows how many of its steps are reviewed", () => {
    render(<FlowIntroBlock flow={f1} dependencies={[]} ticked={["f1.1", "f1.2", "f2.1"]} />);
    expect(screen.getByText("2 of 10 steps reviewed")).toBeInTheDocument();
  });

  it("marks the whole flow reviewed and back through the callback", async () => {
    const onSetReviewed = vi.fn();
    const { rerender } = render(
      <FlowIntroBlock flow={f2!} dependencies={[]} ticked={["f2.1"]} onSetReviewed={onSetReviewed} />,
    );
    const button = screen.getByRole("button", { name: /Mark flow reviewed/ });
    expect(button).toHaveAttribute("aria-pressed", "false");
    expect(button).toHaveAccessibleDescription(/all steps of this flow/i);
    await userEvent.click(button);
    expect(onSetReviewed).toHaveBeenLastCalledWith(true);
    rerender(
      <FlowIntroBlock flow={f2!} dependencies={[]} ticked={["f2.1", "f2.2"]} onSetReviewed={onSetReviewed} />,
    );
    expect(screen.getByText("2 of 2 steps reviewed")).toBeInTheDocument();
    expect(button).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(button);
    expect(onSetReviewed).toHaveBeenLastCalledWith(false);
  });

  it("offers no tick button without a callback", () => {
    render(<FlowIntroBlock flow={f1} dependencies={[]} ticked={[]} />);
    expect(screen.queryByRole("button", { name: /Mark flow reviewed/ })).not.toBeInTheDocument();
  });

  it("labels LLM-classified refactors and keeps them a normal flow", () => {
    const flow = structuredClone(f1);
    flow.group = "llm_classified_refactor";
    flow.llm.classification = "refactor_pure";
    render(<FlowIntroBlock flow={flow} dependencies={[]} ticked={[]} />);
    expect(screen.getByText("LLM-classified refactor")).toBeInTheDocument();
    expect(screen.getByText("Problem (before this PR)")).toBeVisible();
  });

  it.each(FIXTURE_STATES)("renders every flow and keeps the wording rules in state %s", (state) => {
    const stateTour = seedFor(state, "tour") as Tour;
    const { ticked_steps } = seedFor(state, "review-state") as ReviewState;
    for (const flow of stateTour.flows) {
      const { container, unmount } = render(
        <FlowIntroBlock flow={flow} dependencies={[]} ticked={ticked_steps} onSetReviewed={() => {}} />,
      );
      expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent(flow.llm.title);
      expect(screen.getByText(/of \d+ steps? reviewed/)).toBeInTheDocument();
      expectTrustWording(container.textContent ?? "");
      unmount();
    }
  });
});

it("shows pending planned ranges without an assessment or review tick", () => {
  const pending = {
    ...f1,
    steps: [],
    analysis_status: { state: "not_analysed", started_at: null, message: null },
    planned: {
      intent: "Bound RPC retries",
      importance_rank: 1,
      ranges: [{ file: "src/client.rs", side: "head", ranges: [{ start_line: 4, end_line: 8 }] }],
    },
  } as Tour["flows"][number];
  render(<FlowIntroBlock flow={pending} dependencies={[]} ticked={[]} />);
  expect(screen.getByText("Not analysed yet")).toBeVisible();
  expect(screen.getByText("Bound RPC retries")).toBeVisible();
  expect(screen.getByText(/src\/client.rs/)).toBeVisible();
  expect(screen.queryByRole("button", { name: /Mark flow reviewed/ })).not.toBeInTheDocument();
  expect(screen.queryByText("▲ HIGH")).not.toBeInTheDocument();
});
