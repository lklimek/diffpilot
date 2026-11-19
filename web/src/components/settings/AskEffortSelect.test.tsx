import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { AskEffortSelect } from "./AskEffortSelect";

const EFFORTS = ["low", "medium", "high"] as const;

describe("AskEffortSelect", () => {
  it("defaults to medium when the server reports none, and reports a change", async () => {
    const onChange = vi.fn();
    render(<AskEffortSelect efforts={[...EFFORTS]} busy={false} onChange={onChange} />);
    const select = screen.getByLabelText("Ask effort");
    expect(select).toHaveValue("medium");
    expect(select).toHaveAccessibleDescription(/everyone using this installation/);
    expect(select).toHaveAccessibleDescription(/answers a question/);
    await userEvent.selectOptions(select, "low");
    expect(onChange).toHaveBeenCalledWith("low");
  });

  it("ignores changes while saving and reports the outcome", async () => {
    const busyChange = vi.fn();
    const { rerender } = render(
      <AskEffortSelect effort="high" efforts={[...EFFORTS]} busy onChange={busyChange} />,
    );
    const select = screen.getByLabelText("Ask effort");
    expect(select).toHaveAttribute("aria-disabled", "true");
    expect(select).not.toBeDisabled();
    await userEvent.selectOptions(select, "low");
    expect(busyChange).not.toHaveBeenCalled();
    expect(select).toHaveValue("high");
    rerender(<AskEffortSelect effort="high" efforts={[...EFFORTS]} busy={false} saved onChange={() => {}} />);
    expect(screen.getByRole("status")).toHaveTextContent("Ask effort saved.");
    rerender(
      <AskEffortSelect
        effort="high"
        efforts={[...EFFORTS]}
        busy={false}
        error="Try again."
        onChange={() => {}}
      />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("The Ask effort was not saved: Try again.");
  });
});
