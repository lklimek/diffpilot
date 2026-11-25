import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import type { WatchedPr } from "../../api/generated/types";
import { clockText } from "../format/time";
import { MenuButton } from "../menu/MenuButton";
import { WatchControl } from "./WatchControl";

const since = "2026-11-25T09:02:00Z";
const assessed = "2026-11-25T10:52:00Z";
const now = new Date("2026-11-25T11:00:00Z");
const watch = {
  since,
  state: "active",
  last_auto_pass: { at: assessed, questions: 1 },
} satisfies Pick<WatchedPr, "since" | "state" | "last_auto_pass">;

it("acts as a checked Watch comments menu item that can stop watching", async () => {
  const user = userEvent.setup();
  const onChange = vi.fn();
  render(
    <MenuButton label="PR actions">
      <WatchControl variant="menuitem" watching onChange={onChange} />
    </MenuButton>,
  );
  await user.click(screen.getByRole("button", { name: "PR actions" }));
  expect(screen.getByRole("menuitemcheckbox", { name: "Watch comments", checked: true })).toHaveFocus();
  await user.keyboard("{Enter}");
  expect(onChange).toHaveBeenCalledWith(false);
});

it("shows when watching started and when the last automatic assessment ran", async () => {
  const onChange = vi.fn();
  render(<WatchControl variant="button" watch={watch} now={now} onChange={onChange} />);
  expect(
    screen.getByText(
      `Watching since ${clockText(since, now)} · Assessed automatically ${clockText(assessed, now)} · 1 question`,
    ),
  ).toBeVisible();
  const control = screen.getByRole("button", { name: "Watching", pressed: true });
  await userEvent.click(control);
  expect(onChange).toHaveBeenCalledWith(false);
});

it("explains the Watch offer in the Comments state and blocks it while saving", async () => {
  const onChange = vi.fn();
  const view = (busy = false) => (
    <WatchControl variant="state" watch={null} now={now} busy={busy} onChange={onChange} />
  );
  const { rerender } = render(view(true));
  const button = screen.getByRole("button", { name: "Watch this PR", pressed: false });
  expect(
    screen.getByText("New comments would be read and assessed automatically; uses LLM tokens."),
  ).toBeVisible();
  expect(button).toHaveAttribute("aria-disabled", "true");
  await userEvent.click(button);
  expect(onChange).not.toHaveBeenCalled();
  rerender(view());
  await userEvent.click(button);
  expect(onChange).toHaveBeenCalledWith(true);
});

it("shows why watching is paused even before an automatic assessment ran", () => {
  render(
    <WatchControl
      variant="state"
      watch={{ ...watch, state: "paused", last_auto_pass: null }}
      now={now}
      onChange={vi.fn()}
    />,
  );
  expect(screen.getByText("Watching paused: sign in again")).toBeVisible();
  expect(screen.getByRole("button", { name: "Watching", pressed: true })).toBeVisible();
});
