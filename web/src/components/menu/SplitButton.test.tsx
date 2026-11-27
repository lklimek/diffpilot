import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { MenuItem } from "./MenuButton";
import { SplitButton } from "./SplitButton";

it("offers independent, keyboard-accessible primary and more actions", async () => {
  const user = userEvent.setup();
  const primary = vi.fn();
  const extra = vi.fn();
  render(
    <SplitButton label="Refresh" icon={<span>↻</span>} onAction={primary}>
      <MenuItem label="Re-judge outdated" onSelect={extra} />
    </SplitButton>,
  );

  const refresh = screen.getByRole("button", { name: "Refresh" });
  const more = screen.getByRole("button", { name: "More refresh options" });
  await user.tab();
  expect(refresh).toHaveFocus();
  await user.keyboard("{Enter}");
  expect(primary).toHaveBeenCalledOnce();
  expect(refresh).not.toHaveAttribute("aria-haspopup");

  await user.tab();
  expect(more).toHaveFocus();
  expect(more).toHaveAttribute("aria-haspopup", "menu");
  await user.keyboard("{ArrowDown}");
  expect(more).toHaveAttribute("aria-expanded", "true");
  expect(screen.getByRole("menuitem", { name: "Re-judge outdated" })).toHaveFocus();
  await user.keyboard("{Escape}");
  expect(more).toHaveFocus();
  expect(more).toHaveAttribute("aria-expanded", "false");
  await user.keyboard("{Enter}{Enter}");
  expect(extra).toHaveBeenCalledOnce();
  expect(primary).toHaveBeenCalledOnce();
});

it("explains an unavailable primary action while leaving the more menu usable", async () => {
  const user = userEvent.setup();
  const primary = vi.fn();
  render(
    <SplitButton label="Refresh" disabledReason="A scan is running." onAction={primary}>
      <MenuItem label="Cancel scan" onSelect={vi.fn()} />
    </SplitButton>,
  );
  const refresh = screen.getByRole("button", { name: "Refresh" });
  expect(refresh).toHaveAttribute("aria-disabled", "true");
  expect(refresh).toHaveAccessibleDescription("A scan is running.");
  await user.click(refresh);
  expect(primary).not.toHaveBeenCalled();
  await user.click(screen.getByRole("button", { name: "More refresh options" }));
  expect(screen.getByRole("menuitem", { name: "Cancel scan" })).toHaveFocus();
});
