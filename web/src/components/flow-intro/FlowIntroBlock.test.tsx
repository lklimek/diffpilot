import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Tour } from "../../api/generated/types";
import { expectTrustWording, toursByState } from "../../__tests__/wording";
import { seedFor } from "../../mocks/fixtures";
import { FlowIntroBlock } from "./FlowIntroBlock";

const tour = seedFor("default", "tour") as Tour;
const [f1, , f3, , f5] = tour.flows as [Tour["flows"][number], ...Tour["flows"]];

describe("FlowIntroBlock", () => {
  it("shows title, LLM risk, the three introduction fields and the LLM line", () => {
    render(<FlowIntroBlock flow={f1} dependencies={[f3!, f5!]} />);
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("1 Send an RPC request with retry");
    const llm = screen.getByRole("region", { name: "LLM analysis" });
    expect(within(llm).getByText("LLM analysis unless marked otherwise.")).toBeInTheDocument();
    expect(within(llm).getByText("Problem (before this PR)")).toBeInTheDocument();
    expect(within(llm).getByText("Change (what this PR does)")).toBeInTheDocument();
    expect(within(llm).getByText("Before / After example")).toBeInTheDocument();
    expect(llm).toHaveTextContent(f1.llm.introduction.example_before_after);
    expect(llm).toHaveTextContent("Flow 3: New error variant RpcError::Exhausted");
    expect(screen.getByText("▲ HIGH")).toBeInTheDocument();
    expect(screen.queryByText("Was wrong")).not.toBeInTheDocument();
  });

  it("lists the deterministic facts apart from the LLM content", () => {
    render(<FlowIntroBlock flow={f1} dependencies={[]} />);
    const facts = screen.getByRole("region", { name: "Facts" });
    expect(facts).toHaveTextContent("10 steps");
    for (const file of f1.facts.files) expect(facts).toHaveTextContent(file);
  });

  it("labels LLM-classified refactors and keeps them a normal flow", () => {
    const flow = structuredClone(f1);
    flow.group = "llm_classified_refactor";
    flow.llm.classification = "refactor_pure";
    render(<FlowIntroBlock flow={flow} dependencies={[]} />);
    expect(screen.getByText("LLM-classified refactor")).toBeInTheDocument();
    expect(screen.getByText("Problem (before this PR)")).toBeVisible();
  });

  it.each(toursByState)("keeps the wording rules in state %s", (_, stateTour) => {
    for (const flow of stateTour.flows) {
      const { container, unmount } = render(<FlowIntroBlock flow={flow} dependencies={[]} />);
      expectTrustWording(container.textContent ?? "");
      unmount();
    }
  });
});
