import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { HEAD_BUTTON } from "./button";
import { Tooltip } from "../tooltip/Tooltip";
import { HideButton } from "./HideButton";

const panel = (onHide = vi.fn()) => {
  render(
    <>
      <Tooltip tip="Shows the panel again.">
        <button type="button">Show the panel</button>
      </Tooltip>
      <HideButton
        name="Hide the panel"
        tip="Hides the panel."
        onHide={onHide}
        focusAfter={() => screen.getByRole("button", { name: "Show the panel" })}
      />
    </>,
  );
  return { onHide, hide: screen.getByRole("button", { name: "Hide the panel" }) };
};

it("reads Hide, is named after its panel, explains itself in a tooltip and looks like a column head's Hide", () => {
  const { hide } = panel();
  expect(hide).toHaveTextContent(/^Hide$/);
  expect(hide).toHaveAccessibleDescription("Hides the panel.");
  expect(hide.className).toBe(HEAD_BUTTON);
});

it("by keyboard, the focus goes to the control that shows the panel again (the button leaves with its panel)", async () => {
  const { onHide, hide } = panel();
  hide.focus();
  await userEvent.keyboard("{Enter}");
  expect(onHide).toHaveBeenCalledOnce();
  expect(screen.getByRole("button", { name: "Show the panel" })).toHaveFocus();
});

it("a click leaves the focus alone, so the showing control's tooltip does not open under the pointer", async () => {
  const { onHide, hide } = panel();
  await userEvent.click(hide);
  expect(onHide).toHaveBeenCalledOnce();
  expect(screen.getByRole("button", { name: "Show the panel" })).not.toHaveFocus();
  expect(screen.queryByRole("tooltip", { name: "" })).toBeNull();
  expect(screen.queryByText("Shows the panel again.")).not.toBeVisible();
});
