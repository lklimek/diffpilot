import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { AccountCard } from "./AccountCard";

it("names the signed-in login and places sign-out-everywhere behind confirmation", () => {
  render(<AccountCard login="alice" busy={false} onSignOutEverywhere={vi.fn()} />);
  expect(screen.getByRole("region", { name: "GitHub sign-in" })).toHaveTextContent("Signed in as alice");
  expect(screen.queryByRole("button", { name: "Sign out" })).toBeNull();
});

it("busy: inactive; an error is said", async () => {
  const onSignOutEverywhere = vi.fn();
  render(
    <AccountCard
      login="alice"
      busy
      error="The server did not answer."
      onSignOutEverywhere={onSignOutEverywhere}
    />,
  );
  const button = screen.getByRole("button", { name: "Sign out everywhere" });
  expect(button).toHaveAttribute("aria-disabled", "true");
  await userEvent.click(button);
  expect(onSignOutEverywhere).not.toHaveBeenCalled();
  expect(screen.getByRole("alert")).toHaveTextContent("Not signed out: The server did not answer.");
});

it("Sign out everywhere asks first, says what it ends, and Cancel keeps everything", async () => {
  const onSignOutEverywhere = vi.fn();
  render(<AccountCard login="alice" busy={false} onSignOutEverywhere={onSignOutEverywhere} />);
  const button = screen.getByRole("button", { name: "Sign out everywhere" });
  expect(button).toHaveAccessibleDescription(/Asks first/);
  await userEvent.click(button);
  expect(onSignOutEverywhere).not.toHaveBeenCalled();
  const dialog = screen.getByRole("alertdialog", { name: "Sign out everywhere?" });
  expect(dialog).toHaveAccessibleDescription(
    "This ends every session of yours in every browser, revokes all your personal access tokens and deletes the GitHub sign-in this server keeps for the nightly scan; the nightly scan then waits until you sign in again. Nothing changes on GitHub.",
  );
  expect(within(dialog).getByRole("button", { name: "Cancel" })).toHaveFocus();
  await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
  expect(screen.queryByRole("alertdialog")).toBeNull();
  expect(button).toHaveFocus();
  await userEvent.click(button);
  await userEvent.keyboard("{Escape}");
  expect(screen.queryByRole("alertdialog")).toBeNull();
  expect(button).toHaveFocus();
  await userEvent.click(button);
  await userEvent.click(
    within(screen.getByRole("alertdialog")).getByRole("button", { name: "Yes, sign out everywhere" }),
  );
  expect(onSignOutEverywhere).toHaveBeenCalledOnce();
});
