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
    expect(select).toHaveAccessibleDescription(/for everyone using this installation/);
    await userEvent.selectOptions(select, "medium");
    expect(onChange).toHaveBeenCalledWith("medium");
  });

  it("is disabled while saving and reports the outcome", () => {
    const { rerender } = render(
      <JudgeEffortSelect effort="high" efforts={[...EFFORTS]} busy onChange={() => {}} />,
    );
    expect(screen.getByLabelText("Dashboard ranking effort")).toBeDisabled();
    expect(screen.getByLabelText("Dashboard ranking effort")).toHaveValue("high");
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
