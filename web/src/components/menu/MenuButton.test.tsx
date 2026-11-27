import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, useLocation, useNavigate } from "react-router";
import { expect, it, vi } from "vitest";
import { MenuButton, MenuItem } from "./MenuButton";

it("opens by keyboard and uses arrows, Home, End and Escape to manage focus", async () => {
  const user = userEvent.setup();
  render(
    <MenuButton label="Filter">
      <MenuItem label="All" selection={{ kind: "radio", checked: true }} onSelect={vi.fn()} />
      <MenuItem label="Mine" selection={{ kind: "radio", checked: false }} onSelect={vi.fn()} />
      <MenuItem label="Watch comments" selection={{ kind: "checkbox", checked: true }} onSelect={vi.fn()} />
    </MenuButton>,
  );
  const trigger = screen.getByRole("button", { name: "Filter" });
  expect(trigger).toHaveAttribute("aria-haspopup", "menu");
  expect(trigger).toHaveAttribute("aria-expanded", "false");

  await user.tab();
  await user.keyboard("{ArrowUp}");
  const menu = screen.getByRole("menu", { name: "Filter" });
  expect(trigger).toHaveAttribute("aria-expanded", "true");
  expect(trigger).toHaveAttribute("aria-controls", menu.id);
  const all = screen.getByRole("menuitemradio", { name: "All", checked: true });
  const mine = screen.getByRole("menuitemradio", { name: "Mine", checked: false });
  const watching = screen.getByRole("menuitemcheckbox", { name: "Watch comments", checked: true });
  expect(watching).toHaveFocus();

  await user.keyboard("{ArrowDown}");
  expect(all).toHaveFocus();
  await user.keyboard("{ArrowDown}");
  expect(mine).toHaveFocus();
  await user.keyboard("{End}");
  expect(watching).toHaveFocus();
  await user.keyboard("{Home}");
  expect(all).toHaveFocus();
  await user.keyboard("{ArrowUp}");
  expect(watching).toHaveFocus();
  await user.keyboard("{Escape}");
  expect(trigger).toHaveFocus();
  expect(trigger).toHaveAttribute("aria-expanded", "false");
  expect(screen.queryByRole("menu")).toBeNull();

  await user.keyboard(" ");
  expect(screen.getByRole("menuitemradio", { name: "All", checked: true })).toHaveFocus();
});

it("keeps a disabled item focusable and announces its reason without activating it", async () => {
  const user = userEvent.setup();
  const unavailable = vi.fn();
  const select = vi.fn();
  render(
    <MenuButton label="Actions">
      <MenuItem
        label="Refresh"
        description="Reads GitHub."
        disabledReason="A scan is running."
        onSelect={unavailable}
      />
      <MenuItem label="Details" description="Shows the last scan." onSelect={select} />
    </MenuButton>,
  );
  const trigger = screen.getByRole("button", { name: "Actions" });
  await user.tab();
  await user.keyboard("{Enter}");
  const disabled = screen.getByRole("menuitem", { name: "Refresh" });
  expect(disabled).toHaveFocus();
  expect(disabled).toHaveAttribute("aria-disabled", "true");
  expect(disabled).toHaveAccessibleDescription("Reads GitHub. A scan is running.");
  await user.keyboard("{Enter} ");
  await user.click(disabled);
  expect(unavailable).not.toHaveBeenCalled();
  expect(trigger).toHaveAttribute("aria-expanded", "true");

  await user.keyboard("{ArrowDown}");
  expect(screen.getByRole("menuitem", { name: "Details" })).toHaveAccessibleDescription(
    "Shows the last scan.",
  );
  await user.keyboard("{Enter}");
  expect(select).toHaveBeenCalledOnce();
  expect(trigger).toHaveFocus();
  expect(screen.queryByRole("menu")).toBeNull();
});

it("closes when Tab or a click moves outside, preserving the new focus", async () => {
  const user = userEvent.setup();
  render(
    <>
      <MenuButton label="Actions">
        <MenuItem label="Open" href="/review" />
      </MenuButton>
      <button type="button">Elsewhere</button>
    </>,
  );
  const trigger = screen.getByRole("button", { name: "Actions" });
  const elsewhere = screen.getByRole("button", { name: "Elsewhere" });
  await user.click(trigger);
  expect(screen.getByRole("menuitem", { name: "Open" })).toHaveAttribute("href", "/review");
  await user.tab();
  expect(elsewhere).toHaveFocus();
  expect(screen.queryByRole("menu")).toBeNull();
  await user.click(trigger);
  await user.click(elsewhere);
  expect(elsewhere).toHaveFocus();
  expect(screen.queryByRole("menu")).toBeNull();
  await user.click(trigger);
  await user.click(trigger);
  expect(screen.queryByRole("menu")).toBeNull();
});

it("closes when a trigger click follows a blur without a focus destination", async () => {
  render(
    <MenuButton label="Actions">
      <MenuItem label="Details" onSelect={vi.fn()} />
    </MenuButton>,
  );
  const trigger = screen.getByRole("button", { name: "Actions" });
  await userEvent.click(trigger);
  const item = screen.getByRole("menuitem", { name: "Details" });

  fireEvent.pointerDown(trigger);
  fireEvent.focusOut(item, { relatedTarget: null });
  fireEvent.click(trigger);
  expect(trigger).toHaveAttribute("aria-expanded", "false");
  expect(screen.queryByRole("menu")).toBeNull();
});

it("opens a GitHub menu link in a new tab and names that behaviour", async () => {
  render(
    <MenuButton label="Actions">
      <MenuItem label="Open on GitHub" href="https://github.com/acme/repo/pull/1" external />
    </MenuButton>,
  );
  await userEvent.click(screen.getByRole("button", { name: "Actions" }));
  const link = screen.getByRole("menuitem", { name: "Open on GitHub, new tab" });
  expect(link).toHaveAttribute("href", "https://github.com/acme/repo/pull/1");
  expect(link).toHaveAttribute("target", "_blank");
  expect(link).toHaveAttribute("rel", "noopener noreferrer");
});

it("follows an in-app link through the router", async () => {
  function Actions() {
    const navigate = useNavigate();
    const { pathname } = useLocation();
    return (
      <>
        <MenuButton label="Actions">
          <MenuItem label="Settings" href="/settings" onNavigate={navigate} />
        </MenuButton>
        <p>{pathname}</p>
      </>
    );
  }
  render(
    <MemoryRouter>
      <Actions />
    </MemoryRouter>,
  );
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Actions" }));
  const link = screen.getByRole("menuitem", { name: "Settings" });
  expect(link).toHaveAttribute("href", "/settings");
  expect(link).not.toHaveAttribute("target");
  await user.keyboard("{Enter}");
  expect(screen.getByText("/settings")).toBeVisible();
  expect(screen.queryByRole("menu")).toBeNull();
});
