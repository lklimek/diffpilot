import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { JudgeEffortSelect } from "./JudgeEffortSelect";

const EFFORTS = ["low", "medium", "high"] as const;

describe("JudgeEffortSelect (plan §F.6 G13)", () => {
  it("defaults to low when the server reports none, and reports a change", async () => {
    const onChange = vi.fn();
    render(<JudgeEffortSelect efforts={[...EFFORTS]} busy={false} onChange={onChange} />);
    const select = screen.getByLabelText("Dashboard ranking effort");
    expect(select).toHaveValue("low");
    expect(select).toHaveAccessibleDescription(/everyone using this installation/);
    expect(select).toHaveAccessibleDescription(/How much reasoning/);
    await userEvent.selectOptions(select, "medium");
    expect(onChange).toHaveBeenCalledWith("medium");
  });

  it("ignores changes while saving and reports the outcome", async () => {
    const busyChange = vi.fn();
    const { rerender } = render(
      <JudgeEffortSelect effort="high" efforts={[...EFFORTS]} busy onChange={busyChange} />,
    );
    const select = screen.getByLabelText("Dashboard ranking effort");
    // Stays focusable while saving (aria-disabled): changes are ignored, focus is kept.
    expect(select).toHaveAttribute("aria-disabled", "true");
    expect(select).not.toBeDisabled();
    expect(select).toHaveValue("high");
    await userEvent.selectOptions(select, "low");
    expect(busyChange).not.toHaveBeenCalled();
    rerender(
      <JudgeEffortSelect effort="high" efforts={[...EFFORTS]} busy={false} saved onChange={() => {}} />,
    );
    expect(screen.getByRole("status")).toHaveTextContent("Ranking effort saved.");
    rerender(
      <JudgeEffortSelect
        effort="high"
        efforts={[...EFFORTS]}
        busy={false}
        error="Try again."
        onChange={() => {}}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("The ranking effort was not saved: Try again.");
  });
});
