import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { AccountCard } from "./AccountCard";

it("names the signed-in login; Sign out has a tooltip saying what it clears", async () => {
  const onSignOut = vi.fn();
  render(<AccountCard login="alice" busy={false} onSignOut={onSignOut} />);
  expect(screen.getByRole("region", { name: "Account" })).toHaveTextContent("Signed in as alice");
  const button = screen.getByRole("button", { name: "Sign out" });
  expect(button).toHaveAccessibleDescription(/unsent comment drafts in this tab/);
  expect(button).toHaveAccessibleDescription(/other browsers stay signed in/);
  await userEvent.click(button);
  expect(onSignOut).toHaveBeenCalledOnce();
});

it("busy: inactive; an error is said", async () => {
  const onSignOut = vi.fn();
  render(<AccountCard login="alice" busy error="The server did not answer." onSignOut={onSignOut} />);
  await userEvent.click(screen.getByRole("button", { name: "Signing out…" }));
  expect(onSignOut).not.toHaveBeenCalled();
  expect(screen.getByRole("alert")).toHaveTextContent("Not signed out: The server did not answer.");
});
