import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Tour } from "../../api/generated/types";
import { expectTrustWording, toursByState } from "../../__tests__/wording";
import { seedFor } from "../../mocks/fixtures";
import { NavList } from "./NavList";

const tour = seedFor("default", "tour") as Tour;

describe("NavList", () => {
  it("lists PR context and the flows in tour order; marks the PR context as current", () => {
    render(<NavList flows={tour.flows} current={{ block: "pr_context" }} onSelect={() => {}} />);
    expect(screen.getByRole("button", { name: "PR context" })).toHaveAttribute("aria-current", "step");
    const flows = screen.getAllByRole("button", { name: /^Flow \d/ });
    expect(flows.map((b) => b.textContent)).toEqual(
      tour.flows.map((f) => expect.stringContaining(f.llm.title)),
    );
    // No flow is open: steps are listed only for the current flow.
    expect(screen.queryByRole("button", { name: /^Step / })).not.toBeInTheDocument();
  });

  it("opens the current flow's numbered steps and marks the current step", () => {
    render(<NavList flows={tour.flows} current={{ block: "step", step_id: "f1.5" }} onSelect={() => {}} />);
    const steps = screen.getAllByRole("button", { name: /^Step / });
    expect(steps).toHaveLength(10);
    expect(steps[4]).toHaveAccessibleName("Step 5: Retry allowed?");
    expect(steps[4]).toHaveAttribute("aria-current", "step");
    expect(screen.getByRole("button", { name: /^Flow 1/ })).not.toHaveAttribute("aria-current");
  });

  it("shows each flow's LLM risk label and selects blocks through the callback", async () => {
    const onSelect = vi.fn();
    render(
      <NavList flows={tour.flows} current={{ block: "flow_intro", flow_id: "f1" }} onSelect={onSelect} />,
    );
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

  it.each(toursByState)("keeps the wording rules in state %s", (_, stateTour) => {
    const { container } = render(
      <NavList flows={stateTour.flows} current={{ block: "step", step_id: "f3.1" }} onSelect={() => {}} />,
    );
    expectTrustWording(container.textContent ?? "");
  });
});
