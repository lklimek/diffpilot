import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import type { ControlledOwners } from "../../api/generated/types";
import { ControlledOwnersCard } from "./ControlledOwnersCard";

const at = new Date(2026, 10, 20, 9, 30).toISOString();
const SAVED: ControlledOwners = { owners: ["dashpay"], changed_by: "lklimek", changed_at: at };
const NONE: ControlledOwners = { owners: [], changed_by: null, changed_at: null };

const card = () => screen.getByRole("region", { name: "Controlled organisations" });
const field = () => screen.getByLabelText("Add a controlled organisation");

describe("ControlledOwnersCard (plan U4: installation-wide list, PR owner implicit)", () => {
  it("lists the owners, who changed them and when, and says the PR's owner is always yours", () => {
    render(<ControlledOwnersCard owners={SAVED} busy={false} onSave={() => {}} />);
    expect(screen.getByRole("list", { name: "Controlled organisations" })).toHaveTextContent("dashpay");
    expect(card()).toHaveTextContent("changed by lklimek at 20 Nov 2026, 09:30");
    expect(card()).toHaveTextContent("A pull request's own owner is always yours; it need not be listed.");
    expect(screen.getByRole("link", { name: "Help: Controlled organisations" })).toBeVisible();
    expectTrustWording(card().textContent ?? "");
  });

  it("empty: says only each pull request's own owner is yours", () => {
    render(<ControlledOwnersCard owners={NONE} busy={false} onSave={() => {}} />);
    expect(card()).toHaveTextContent("None listed: only each pull request's own owner is yours.");
    expect(card()).not.toHaveTextContent("changed by");
  });

  it("adds and removes owners, then saves the edited list for the installation", async () => {
    const onSave = vi.fn();
    render(<ControlledOwnersCard owners={SAVED} busy={false} onSave={onSave} />);
    const save = screen.getByRole("button", { name: "Save organisations" });
    expect(save).toHaveAccessibleDescription(/everyone using this installation/);
    expect(save).toHaveAttribute("aria-disabled", "true");
    await userEvent.type(field(), "acme-labs{Enter}");
    await userEvent.click(screen.getByRole("button", { name: "Remove dashpay" }));
    expect(save).not.toHaveAttribute("aria-disabled");
    await userEvent.click(save);
    expect(onSave).toHaveBeenCalledWith(["acme-labs"]);
  });

  it.each([
    ["not a login", 'Controlled organisations: "not a login" is not a GitHub login'],
    ["-dash", 'Controlled organisations: "-dash" is not a GitHub login'],
    ["a".repeat(40), "is not a GitHub login"],
    ["DashPay", 'Controlled organisations: "DashPay" is already listed.'],
  ])("refuses %s with a reason", async (value, reason) => {
    const onSave = vi.fn();
    render(<ControlledOwnersCard owners={SAVED} busy={false} onSave={onSave} />);
    await userEvent.type(field(), `${value}{Enter}`);
    expect(card()).toHaveTextContent(reason);
    expect(field()).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("list", { name: "Controlled organisations" }).children).toHaveLength(1);
  });

  it("at most 50 owners", async () => {
    const full = { ...NONE, owners: Array.from({ length: 50 }, (_, i) => `org${i}`) };
    render(<ControlledOwnersCard owners={full} busy={false} onSave={() => {}} />);
    await userEvent.type(field(), "one-more{Enter}");
    expect(card()).toHaveTextContent("Controlled organisations: at most 50.");
  });

  it("while saving the Save ignores clicks; the outcome is reported", async () => {
    const onSave = vi.fn();
    const { rerender } = render(<ControlledOwnersCard owners={NONE} busy onSave={onSave} />);
    await userEvent.type(field(), "dashpay{Enter}");
    const save = screen.getByRole("button", { name: "Save organisations" });
    expect(save).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(save);
    expect(onSave).not.toHaveBeenCalled();
    rerender(<ControlledOwnersCard owners={SAVED} busy={false} saved onSave={onSave} />);
    expect(screen.getByRole("status")).toHaveTextContent("Controlled organisations saved.");
    rerender(<ControlledOwnersCard owners={SAVED} busy={false} error="Try again." onSave={onSave} />);
    expect(screen.getByRole("alert")).toHaveTextContent(
      "The controlled organisations were not saved: Try again.",
    );
  });
});
