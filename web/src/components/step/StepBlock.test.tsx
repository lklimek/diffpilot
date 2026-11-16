import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { Step, Tour } from "../../api/generated/types";
import { expectTrustWording, toursByState } from "../../__tests__/wording";
import { seedFor } from "../../mocks/fixtures";
import { findStep } from "../../screens/tour/sequence";
import { StepBlock } from "./StepBlock";

const stepOf = (tour: Tour, id: string): Step => findStep(tour, id)!.step;
const tour = seedFor("default", "tour") as Tour;

describe("StepBlock", () => {
  it("shows the header with a GitHub line link at the head SHA in a new tab", () => {
    const step = stepOf(tour, "f1.5");
    render(<StepBlock step={step} code={<p>code slot</p>} />);
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("5 Retry allowed?");
    const header = screen.getByRole("heading", { level: 2 }).closest("header")!;
    expect(header).toHaveTextContent("src/rpc/client.rs");
    expect(header).toHaveTextContent("RpcClient::send");
    expect(within(header).getByText("added")).toHaveAttribute("data-llm-label");
    const link = screen.getByRole("link", { name: /L99/ });
    expect(link).toHaveAttribute("href", step.facts.github_url);
    expect(link).toHaveAttribute("target", "_blank");
    expect(screen.getByText("code slot")).toBeInTheDocument();
  });

  it("always shows the four facts: called by, decides or changes, goes next, why", () => {
    const step = stepOf(tour, "f1.5");
    render(<StepBlock step={step} code={null} />);
    const llm = screen.getByRole("region", { name: "LLM analysis" });
    expect(within(llm).getByText("LLM analysis unless marked otherwise.")).toBeInTheDocument();
    for (const [term, value] of [
      ["Called by", step.llm.called_by],
      ["Decides or changes", step.llm.decides],
      ["Goes next", step.llm.next],
      ["Why", step.llm.why],
    ]) {
      const dt = within(llm).getByText(term!);
      expect(dt.nextElementSibling).toHaveTextContent(value!);
    }
  });

  it("lists questions to check as questions, marking inferences", () => {
    render(<StepBlock step={stepOf(tour, "f1.5")} code={null} />);
    const questions = screen.getByRole("list", { name: "Questions to check" });
    expect(within(questions).getAllByRole("listitem")).toHaveLength(1);
    expect(questions).toHaveTextContent("can broadcast_tx be sent twice when the reply arrives late?");
    expect(questions).toHaveTextContent("inference");
  });

  it("keeps the assessment collapsed until clicked", async () => {
    render(<StepBlock step={stepOf(tour, "f1.5")} code={null} />);
    const summary = screen.getByText("Assessment (LLM)");
    const details = summary.closest("details")!;
    expect(details).not.toHaveAttribute("open");
    expect(within(details).getByText(/wallet user could see one payment broadcast twice/)).not.toBeVisible();
    await userEvent.click(summary);
    expect(details).toHaveAttribute("open");
    expect(within(details).getByText("▲ HIGH")).toBeVisible();
    expect(details).toHaveTextContent("Smallest correction");
  });

  it("shows the deterministic facts of the step apart from the LLM content", () => {
    render(<StepBlock step={stepOf(tour, "f5.1")} code={null} />);
    const facts = screen.getByRole("region", { name: "Facts" });
    expect(facts).toHaveTextContent("Lines 38–95");
    expect(facts).toHaveTextContent("2 changed lines");
    expect(within(facts).getByText("≠ 2 lines differ")).toHaveAttribute("data-badge");
    expect(facts).toHaveTextContent("Moved from src/util/backoff.rs:11–68");
  });

  it("names removed code of a deleted site at the base commit", () => {
    render(<StepBlock step={stepOf(tour, "f4.1")} code={null} />);
    expect(screen.getByRole("region", { name: "Facts" })).toHaveTextContent("base commit");
    expect(screen.getByText("deleted site")).toBeInTheDocument();
  });

  it("says so when the LLM raised no questions and gave no assessment (noq)", () => {
    const noq = seedFor("noq", "tour") as Tour;
    render(<StepBlock step={stepOf(noq, "f1.5")} code={null} />);
    expect(
      screen.getByText("The LLM raised no questions. That is not evidence of no defects."),
    ).toBeInTheDocument();
    expect(screen.getByText("The LLM gave no assessment for this step.")).toBeInTheDocument();
  });

  it("shows the confidently wrong HIGH of state wrong only as an LLM rating", async () => {
    const wrong = seedFor("wrong", "tour") as Tour;
    render(<StepBlock step={stepOf(wrong, "f3.1")} code={null} />);
    await userEvent.click(screen.getByText("Assessment (LLM)"));
    expect(screen.getByText("▲ HIGH")).toHaveAccessibleDescription(/LLM rating/);
  });

  it.each(toursByState)("keeps the wording rules on every step in state %s", (_, stateTour) => {
    for (const step of stateTour.flows.flatMap((f) => f.steps)) {
      const { container, unmount } = render(<StepBlock step={step} code={null} />);
      expectTrustWording(container.textContent ?? "");
      unmount();
    }
  });
});
