import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { FlowWork } from "./FlowWork";

it("lets the user stop background work without changing review ticks", async () => {
  const onBackground = vi.fn();
  const { rerender } = render(
    <FlowWork
      done={2}
      total={5}
      background
      busy={false}
      onBackground={onBackground}
      progressHref="/a/progress"
    />,
  );
  expect(screen.getByText("2 of 5 flows analysed. Pending flows have not been assessed.")).toBeVisible();
  const toggle = screen.getByRole("checkbox", { name: "Analyse the remaining flows in the background" });
  await userEvent.click(toggle);
  expect(onBackground).toHaveBeenCalledWith(false);
  onBackground.mockClear();
  rerender(
    <FlowWork
      done={2}
      total={5}
      background={false}
      busy
      onBackground={onBackground}
      progressHref="/a/progress"
    />,
  );
  expect(toggle).toHaveAttribute("aria-disabled", "true");
  await userEvent.click(toggle);
  expect(onBackground).not.toHaveBeenCalled();
});
