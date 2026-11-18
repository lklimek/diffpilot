import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { FlowWork } from "./FlowWork";

const props = {
  done: 2,
  total: 5,
  remainingLines: 140,
  parked: false,
  busy: false,
  progressHref: "/a/progress",
};

it("off: one button analyses all remaining flows, with their count and lines as a hint", async () => {
  const onBackground = vi.fn();
  render(<FlowWork {...props} background={false} onBackground={onBackground} />);
  expect(screen.getByText("2 of 5 flows analysed. Pending flows have not been assessed.")).toBeVisible();
  const button = screen.getByRole("button", { name: "Analyse all remaining flows" });
  expect(button).toHaveAccessibleDescription(/3 flows not analysed yet, 140 changed lines/);
  await userEvent.click(button);
  expect(onBackground).toHaveBeenCalledWith(true);
});

it("on: says so and offers Stop; busy keeps it focusable but inactive", async () => {
  const onBackground = vi.fn();
  const { rerender } = render(<FlowWork {...props} background onBackground={onBackground} />);
  const stop = screen.getByRole("button", { name: "Analysing all remaining flows — Stop" });
  await userEvent.click(stop);
  expect(onBackground).toHaveBeenCalledWith(false);
  onBackground.mockClear();
  rerender(<FlowWork {...props} background busy onBackground={onBackground} />);
  expect(stop).toHaveAttribute("aria-disabled", "true");
  await userEvent.click(stop);
  expect(onBackground).not.toHaveBeenCalled();
});

it("parked: says that nothing is being analysed and what to do", () => {
  render(<FlowWork {...props} background={false} parked onBackground={() => {}} />);
  expect(screen.getByRole("status")).toHaveTextContent(
    "Nothing is being analysed. Choose a flow to analyse, or analyse all remaining flows.",
  );
});

it("with every flow analysed: no button", () => {
  render(<FlowWork {...props} done={5} remainingLines={0} background={false} onBackground={() => {}} />);
  expect(screen.queryByRole("button", { name: /all remaining flows/ })).toBeNull();
});
