import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { AccountCard } from "./AccountCard";

it("names the signed-in login; Sign out has a tooltip saying what it clears", async () => {
  const onSignOut = vi.fn();
  render(<AccountCard login="alice" busy={false} onSignOut={onSignOut} onSignOutEverywhere={vi.fn()} />);
  expect(screen.getByRole("region", { name: "Account" })).toHaveTextContent("Signed in as alice");
  const button = screen.getByRole("button", { name: "Sign out" });
  expect(button).toHaveAccessibleDescription(/unsent comment drafts in this tab/);
  expect(button).toHaveAccessibleDescription(/other browsers stay signed in/);
  await userEvent.click(button);
  expect(onSignOut).toHaveBeenCalledOnce();
});

it("busy: inactive; an error is said", async () => {
  const onSignOut = vi.fn();
  render(
    <AccountCard
      login="alice"
      busy
      error="The server did not answer."
      onSignOut={onSignOut}
      onSignOutEverywhere={vi.fn()}
    />,
  );
  expect(screen.getByRole("button", { name: "Sign out everywhere" })).toHaveAttribute(
    "aria-disabled",
    "true",
  );
  await userEvent.click(screen.getByRole("button", { name: "Signing out…" }));
  expect(onSignOut).not.toHaveBeenCalled();
  expect(screen.getByRole("alert")).toHaveTextContent("Not signed out: The server did not answer.");
});

it("Sign out everywhere asks first, says what it ends, and Cancel keeps everything", async () => {
  const onSignOut = vi.fn();
  const onSignOutEverywhere = vi.fn();
  render(
    <AccountCard
      login="alice"
      busy={false}
      onSignOut={onSignOut}
      onSignOutEverywhere={onSignOutEverywhere}
    />,
  );
  const button = screen.getByRole("button", { name: "Sign out everywhere" });
  expect(button).toHaveAccessibleDescription(/Asks first/);
  await userEvent.click(button);
  expect(onSignOutEverywhere).not.toHaveBeenCalled();
  const dialog = screen.getByRole("alertdialog", { name: "Sign out everywhere?" });
  expect(dialog).toHaveAccessibleDescription(
    "This ends every session of yours in every browser, revokes all your personal access tokens and deletes the GitHub sign-in this server keeps for the automatic refresh; the automatic refresh then waits until you sign in again. Nothing changes on GitHub.",
  );
  expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus();
  await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
  expect(screen.queryByRole("alertdialog")).toBeNull();
  await userEvent.click(button);
  await userEvent.keyboard("{Escape}");
  expect(screen.queryByRole("alertdialog")).toBeNull();
  await userEvent.click(button);
  await userEvent.click(
    within(screen.getByRole("alertdialog")).getByRole("button", { name: "Yes, sign out everywhere" }),
  );
  expect(onSignOutEverywhere).toHaveBeenCalledOnce();
  expect(onSignOut).not.toHaveBeenCalled();
});
