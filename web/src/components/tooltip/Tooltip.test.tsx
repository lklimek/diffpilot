import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { Tooltip } from "./Tooltip";

describe("Tooltip", () => {
  it("describes its trigger and shows on focus and hover", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip tip="Explains the button">
        <button type="button">Go</button>
      </Tooltip>,
    );
    const button = screen.getByRole("button", { name: "Go" });
    expect(button).toHaveAccessibleDescription("Explains the button");
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    await user.tab();
    expect(screen.getByRole("tooltip")).toHaveTextContent("Explains the button");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    await user.hover(button);
    expect(screen.getByRole("tooltip")).toBeVisible();
    await user.unhover(button);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });
});
