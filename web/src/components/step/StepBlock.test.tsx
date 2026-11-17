import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ReviewState, Step, Tour } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { FIXTURE_STATES, seedFor } from "../../mocks/fixtures";
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

  it("leaves questions to check to the inspector (brief §4.3)", () => {
    render(<StepBlock step={stepOf(tour, "f1.5")} code={null} />);
    expect(screen.queryByText(/Questions to check/)).toBeNull();
    expect(screen.queryByText(/can broadcast_tx be sent twice/)).toBeNull();
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

  it("says so when the LLM gave no assessment (noq)", () => {
    const noq = seedFor("noq", "tour") as Tour;
    render(<StepBlock step={stepOf(noq, "f1.5")} code={null} />);
    expect(screen.getByText("The LLM gave no assessment for this step.")).toBeInTheDocument();
  });

  it("shows the confidently wrong HIGH of state wrong only as an LLM rating", async () => {
    const wrong = seedFor("wrong", "tour") as Tour;
    render(<StepBlock step={stepOf(wrong, "f3.1")} code={null} />);
    await userEvent.click(screen.getByText("Assessment (LLM)"));
    expect(screen.getByText("▲ HIGH")).toHaveAccessibleDescription(/LLM rating/);
  });

  it("ticks the step as reviewed and back through the callback", async () => {
    const onSetReviewed = vi.fn();
    const step = stepOf(tour, "f1.5");
    const { rerender } = render(<StepBlock step={step} code={null} onSetReviewed={onSetReviewed} />);
    const button = screen.getByRole("button", { name: /Mark reviewed/ });
    expect(button).toHaveAttribute("aria-pressed", "false");
    expect(button).toHaveAccessibleDescription(/\(x\)/);
    await userEvent.click(button);
    expect(onSetReviewed).toHaveBeenLastCalledWith(true);
    rerender(<StepBlock step={step} code={null} reviewed onSetReviewed={onSetReviewed} />);
    expect(button).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(button);
    expect(onSetReviewed).toHaveBeenLastCalledWith(false);
  });

  it("offers no tick button without a callback", () => {
    render(<StepBlock step={stepOf(tour, "f1.5")} code={null} />);
    expect(screen.queryByRole("button", { name: /Mark reviewed/ })).not.toBeInTheDocument();
  });

  it("draws a context step's code with a distinct border style, as the LLM's classification", () => {
    const { unmount } = render(<StepBlock step={stepOf(tour, "f1.1")} code={<p>ctx code</p>} />);
    const wrap = screen.getByText("ctx code").closest("[data-code-slot]")!;
    expect(wrap).toHaveAttribute("data-context");
    expect(wrap).toHaveClass("border-dotted");
    // "No changed lines" is the code view's caption; the step block does not repeat it.
    expect(wrap).toHaveTextContent(/^Context \(LLM classification\): connects the flow\.ctx code$/);
    unmount();
    render(<StepBlock step={stepOf(tour, "f1.5")} code={<p>code</p>} />);
    const plain = screen.getByText("code").closest("[data-code-slot]")!;
    expect(plain).not.toHaveAttribute("data-context");
    expect(plain).not.toHaveTextContent("Context");
  });

  it("never calls a context step unchanged when the diff changed lines in it", () => {
    const step = structuredClone(stepOf(tour, "f1.1"));
    step.facts.changed_lines = [{ start_line: 129, end_line: 129 }];
    render(<StepBlock step={step} code={<p>ctx code</p>} />);
    const wrap = screen.getByText("ctx code").closest("[data-code-slot]")!;
    expect(wrap).not.toHaveTextContent(/unchanged|No changed lines/i);
    expect(wrap).toHaveTextContent(
      "Context (LLM classification): connects the flow. 1 changed line in this range (from the diff).",
    );
  });

  it("summarises its remark markers: LLM remarks, GitHub threads, own comments", () => {
    render(<StepBlock step={stepOf(tour, "f1.5")} code={null} ownComments={2} />);
    const markers = screen.getByRole("list", { name: "Markers in this step" });
    expect(within(markers).getAllByRole("listitem")).toHaveLength(3);
    for (const text of ["◆ 3 LLM remarks", "1 GitHub thread", "▣ 2 your comments"]) {
      expect(within(markers).getByText(text)).toHaveAttribute("aria-describedby");
    }
    expect(within(markers).getByText("◆ 3 LLM remarks")).toHaveAccessibleDescription(/LLM/);
  });

  it("never lets a partial or unchecked thread list read as complete", () => {
    const { unmount } = render(<StepBlock step={stepOf(tour, "f1.5")} code={null} threadsStatus="partial" />);
    const markers = () => screen.getByRole("list", { name: "Markers in this step" });
    expect(markers()).toHaveTextContent("1 GitHub thread (the list from GitHub is incomplete)");
    unmount();
    const plain = structuredClone(stepOf(tour, "f2.2"));
    render(<StepBlock step={plain} code={null} threadsStatus="not_checked" />);
    expect(markers()).toHaveTextContent("Review threads: not checked");
  });

  it("shows no marker list when the step has no markers", () => {
    const step = structuredClone(stepOf(tour, "f2.2"));
    step.llm.annotations = [];
    render(<StepBlock step={step} code={null} />);
    expect(screen.queryByRole("list", { name: "Markers in this step" })).not.toBeInTheDocument();
  });

  it("lists the same pattern applied elsewhere as a collapsed row with step references", async () => {
    const f3 = tour.flows[2]!;
    const patterns = structuredClone(f3.llm.pattern_repetitions);
    patterns[0]!.sites.push({
      ...patterns[0]!.sites[0]!,
      step_id: "f1.8",
      range: { start_line: 7, end_line: 7 },
    });
    const onSelectStep = vi.fn();
    render(
      <StepBlock step={stepOf(tour, "f3.2")} code={null} patterns={patterns} onSelectStep={onSelectStep} />,
    );
    const summary = screen.getByText("Same pattern × 2");
    const details = summary.closest("details")!;
    expect(details).not.toHaveAttribute("open");
    await userEvent.click(summary);
    expect(details).toHaveTextContent(patterns[0]!.description);
    expect(within(details).getByRole("link", { name: /src\/sync\/peers.rs:201–205/ })).toHaveAttribute(
      "href",
      patterns[0]!.sites[0]!.github_url,
    );
    const open = within(details).getAllByRole("button", { name: /^Open step/ });
    expect(open).toHaveLength(1);
    expect(open[0]).toHaveAccessibleName("Open step for src/sync/peers.rs:7");
    await userEvent.click(open[0]!);
    expect(onSelectStep).toHaveBeenCalledWith("f1.8");
  });

  it("ignores pattern repetitions explained in other steps", () => {
    const f3 = tour.flows[2]!;
    render(<StepBlock step={stepOf(tour, "f3.1")} code={null} patterns={f3.llm.pattern_repetitions} />);
    expect(screen.queryByText(/Same pattern/)).not.toBeInTheDocument();
  });

  it("shows impact outside the diff under the step, with its code from the slot", () => {
    const step = stepOf(tour, "f2.1");
    render(<StepBlock step={step} code={null} impactCode={(impact) => <p>code of {impact.file}</p>} />);
    const impact = screen.getByRole("region", { name: "Impact outside the diff" });
    expect(impact).toHaveTextContent("tests/sync_headers.rs");
    expect(impact).toHaveTextContent(step.llm.impact[0]!.reason);
    expect(impact).toHaveTextContent("fact");
    expect(within(impact).getByText("code of tests/sync_headers.rs")).toBeInTheDocument();
    expect(within(impact).getByRole("link", { name: /L8–14/ })).toHaveAttribute("target", "_blank");
  });

  it("never links a GitHub URL that does not point to github.com (T12)", () => {
    const step = structuredClone(stepOf(tour, "f2.1"));
    step.llm.impact[0]!.github_url = "javascript:alert(1)";
    render(<StepBlock step={step} code={null} />);
    const impact = screen.getByRole("region", { name: "Impact outside the diff" });
    expect(within(impact).queryAllByRole("link")).toEqual([]);
  });

  it("toggles the old code through the callback", async () => {
    const onToggle = vi.fn();
    render(<StepBlock step={stepOf(tour, "f1.2")} code={null} oldCode={{ shown: true, onToggle }} />);
    const button = screen.getByRole("button", { name: /Show old code/ });
    expect(button).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(button);
    expect(onToggle).toHaveBeenCalledOnce();
  });

  it.each(FIXTURE_STATES)("renders every step and keeps the wording rules in state %s", (state) => {
    const stateTour = seedFor(state, "tour") as Tour;
    const { ticked_steps } = seedFor(state, "review-state") as ReviewState;
    for (const flow of stateTour.flows) {
      for (const step of flow.steps) {
        const { container, unmount } = render(
          <StepBlock
            step={step}
            code={<p>code</p>}
            patterns={flow.llm.pattern_repetitions}
            reviewed={ticked_steps.includes(step.id)}
            onSetReviewed={() => {}}
          />,
        );
        expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent(step.llm.title);
        expect(screen.getByRole("button", { name: /Mark reviewed/ })).toHaveAttribute(
          "aria-pressed",
          String(ticked_steps.includes(step.id)),
        );
        expectTrustWording(container.textContent ?? "");
        unmount();
      }
    }
  });
});
