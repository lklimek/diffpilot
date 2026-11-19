import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { HEAD_BUTTON } from "./button";
import { HideButton } from "./HideButton";

it("reads Hide, is named after its panel, explains itself in a tooltip and looks like a column head's Hide", async () => {
  const onHide = vi.fn();
  render(
    <>
      <button type="button">Show the panel</button>
      <HideButton
        name="Hide the panel"
        tip="Hides the panel."
        onHide={onHide}
        focusAfter={() => screen.getByRole("button", { name: "Show the panel" })}
      />
    </>,
  );
  const hide = screen.getByRole("button", { name: "Hide the panel" });
  expect(hide).toHaveTextContent(/^Hide$/);
  expect(hide).toHaveAccessibleDescription("Hides the panel.");
  expect(hide.className).toBe(HEAD_BUTTON);
  await userEvent.click(hide);
  expect(onHide).toHaveBeenCalledOnce();
  // The button leaves with its panel: the focus goes to the control that shows it again.
  expect(screen.getByRole("button", { name: "Show the panel" })).toHaveFocus();
});
