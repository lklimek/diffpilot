import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import type { Tour } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { Inspector } from "./Inspector";
import { StepBlock } from "../step/StepBlock";
import { NavList } from "../nav/NavList";

const tour = seedFor("default", "tour") as Tour;
it("keeps one step list with accessible kinds in navigation", () => {
  render(<Inspector tour={tour} block={{ block: "flow_intro", flow_id: "f1" }} />);
  expect(screen.queryByText("Steps of this flow")).not.toBeInTheDocument();
  render(
    <NavList
      flows={tour.flows}
      quiet={tour.quiet}
      ticked={[]}
      current={{ block: "flow_intro", flow_id: "f1" }}
      onSelect={() => {}}
    />,
  );
  expect(screen.getAllByLabelText(/Step kind:/).length).toBe(tour.flows[0]!.steps.length);
});
it("hides empty inspector sections without claiming safety", () => {
  const data = structuredClone(tour);
  const step = data.flows[0]!.steps[0]!;
  step.llm.annotations = [];
  step.llm.assessment = null;
  step.llm.why = "";
  step.facts.call_hierarchy = null;
  step.facts.thread_ids = [];
  render(<Inspector tour={data} block={{ block: "step", step_id: step.id }} />);
  for (const name of [
    /Behaviour:/,
    /^Why/,
    /Consequences/,
    /Call hierarchy/,
    /Questions to check/,
    /^Assessment/,
    /GitHub threads/,
  ])
    expect(screen.queryByRole("button", { name })).not.toBeInTheDocument();
});
it("shows the decision in the title bar and leaves facts and assessment to the inspector", () => {
  const step = tour.flows[0]!.steps[4]!;
  render(<StepBlock step={step} code={<p>Code occupies the row</p>} />);
  expect(
    within(screen.getByRole("heading", { level: 2 }).closest("header")!).getByText(step.llm.decides),
  ).toBeVisible();
  expect(screen.queryByRole("region", { name: "Facts" })).toBeNull();
  expect(screen.queryByText("Assessment (LLM)")).toBeNull();
  expect(screen.queryByText(/^Lines .* changed lines/)).toBeNull();
});
it("lists flow questions with their destination instead of an unlocated count", async () => {
  const onSelectStep = vi.fn();
  render(
    <Inspector tour={tour} block={{ block: "flow_intro", flow_id: "f1" }} onSelectStep={onSelectStep} />,
  );
  const question = tour.flows[0]!.steps.flatMap((s) =>
    s.llm.annotations.filter((a) => a.type === "question").map((a) => ({ s, a })),
  )[0]!;
  const button = screen.getByRole("button", {
    name: new RegExp(`Step ${question.s.number}.*${question.s.llm.title}`),
  });
  expect(button).toHaveTextContent(question.a.text!);
  await userEvent.click(button);
  expect(onSelectStep).toHaveBeenCalledWith(question.s.id);
});
it("shows evidence below the statement, collapsed until opened", async () => {
  const onEvidence = vi.fn();
  render(<Inspector tour={tour} block={{ block: "step", step_id: "f1.5" }} onEvidence={onEvidence} />);
  const behaviour = screen.getByRole("region", { name: /^Behaviour/ });
  const control = within(behaviour).getByText(/Evidence ·/);
  expect(control.closest("details")).not.toHaveAttribute("open");
  expect(within(behaviour).queryByText(/^Lines \d/)).toBeNull();
  await userEvent.click(control);
  const link = within(behaviour).getAllByRole("button", { name: /^Show / })[0]!;
  await userEvent.click(link);
  expect(onEvidence).toHaveBeenCalled();
});

it("keeps a moved-code fact's evidence collapsed and navigable", async () => {
  const step = tour.flows.flatMap((f) => f.steps).find((s) => s.facts.moved)!;
  const onEvidence = vi.fn();
  render(<Inspector tour={tour} block={{ block: "step", step_id: step.id }} onEvidence={onEvidence} />);
  const toggle = screen.getByRole("button", { name: /Moved code/ });
  if (toggle.getAttribute("aria-expanded") === "false") await userEvent.click(toggle);
  const moved = screen.getByRole("region", { name: /Moved code/ });
  const evidence = within(moved).getByText(/Evidence ·/);
  expect(evidence.closest("details")).not.toHaveAttribute("open");
  await userEvent.click(evidence);
  await userEvent.click(within(moved).getAllByRole("button", { name: /^Show / })[0]!);
  expect(onEvidence).toHaveBeenCalledWith({
    file: step.facts.moved!.from.file,
    side: "base",
    line: step.facts.moved!.from.range.start_line,
  });
});
