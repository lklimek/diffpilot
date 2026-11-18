import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { DashboardSettings, DashboardSettingsView } from "../../../api/generated/types";
import viewJson from "../../../../../fixtures/dashboard/api/settings.json";
import { DashboardSettingsCard } from "./DashboardSettingsCard";

const VIEW = viewJson as unknown as DashboardSettingsView;

function renderCard(view: DashboardSettingsView = VIEW, onSave = vi.fn().mockResolvedValue(view)) {
  render(<DashboardSettingsCard view={view} onSave={onSave} />);
  return { onSave, card: screen.getByRole("region", { name: "Dashboard" }) };
}

const saved = (onSave: ReturnType<typeof vi.fn>) => onSave.mock.calls[0]![0] as DashboardSettings;

describe("DashboardSettingsCard (plan §F.6)", () => {
  it("shows the stored settings with fallbacks as placeholders and unreachable rows marked", () => {
    const { card } = renderCard();
    expect(card).toHaveTextContent("Your login alice is always scanned.");
    expect(within(card).getByRole("list", { name: "Linked logins" })).toHaveTextContent("alice-ci");
    const table = within(card).getByRole("table", { name: "Repositories" });
    expect(within(table).getAllByRole("row")).toHaveLength(7);
    expect(screen.getByLabelText("Short name of acme/platform")).toHaveValue("PLAT");
    expect(screen.getByLabelText("Short name of acme/legacy-tools")).toHaveAttribute(
      "placeholder",
      "legacy-tools",
    );
    expect(screen.getByLabelText("Release branches of acme/consensus")).toHaveValue("release/*, vX.Y-dev");
    expect(screen.getByLabelText("Release branches of acme/platform")).toHaveAttribute(
      "placeholder",
      "main, master, release/*, hotfix/*, vX.Y, vX.Y-dev",
    );
    expect(screen.getByLabelText("Policy path of acme/platform")).toHaveValue("policies/platform.json");
    expect(screen.getByLabelText("Watch issues of acme/node-client")).not.toBeChecked();
    const marks = card.querySelectorAll("[data-mark='unreachable']");
    expect(marks).toHaveLength(1);
    expect(marks[0]!.closest("tr")).toBe(screen.getByLabelText("Repository 6").closest("tr"));
    expect(marks[0]).toHaveAccessibleDescription(/could not see acme\/legacy-tools/);
    expect(within(card).getByRole("list", { name: "MemCan projects" })).toHaveTextContent(
      "platformwallet-uinode-clientconsensusdocs-sitelegacy-tools",
    );
    expect(within(card).getByRole("list", { name: "Extra MemCan projects" })).toHaveTextContent(
      "release-notes",
    );
    expect(screen.getByLabelText("Show my MemCan TODOs")).toBeChecked();
    expect(screen.getByLabelText("Refresh my dashboard automatically every day")).toBeChecked();
    expect(screen.getByLabelText("Time of day")).toHaveValue("06:00");
    expect(screen.getByLabelText("Time zone")).toHaveValue("Europe/Warsaw");
  });

  it("refuses a list entry that breaks its rule and adds a valid one", async () => {
    renderCard();
    const field = screen.getByLabelText("Add a linked login");
    await userEvent.type(field, "alice{Enter}");
    expect(field).toHaveAccessibleDescription(/your own login/);
    await userEvent.clear(field);
    await userEvent.type(field, "bad login{Enter}");
    expect(field).toHaveAccessibleDescription(/not a GitHub login/);
    await userEvent.clear(field);
    await userEvent.type(field, "alice-work");
    await userEvent.click(screen.getByRole("button", { name: "Add to linked logins" }));
    expect(screen.getByRole("list", { name: "Linked logins" })).toHaveTextContent("alice-work");
    expect(field).toHaveValue("");
    await userEvent.click(screen.getByRole("button", { name: "Remove alice-ci" }));
    expect(screen.getByRole("list", { name: "Linked logins" })).not.toHaveTextContent("alice-ci");
  });

  it("does not send a form that breaks a rule: names each field, links the error to its input", async () => {
    const { onSave } = renderCard();
    const alias = screen.getByLabelText("Short name of acme/platform");
    await userEvent.clear(alias);
    await userEvent.type(alias, "-x");
    const days = screen.getByLabelText("Look-back days");
    await userEvent.clear(days);
    await userEvent.type(days, "31");
    await userEvent.click(screen.getByRole("button", { name: "Save dashboard settings" }));
    expect(onSave).not.toHaveBeenCalled();
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("Not saved: fix the marked fields.");
    expect(alert).toHaveTextContent("Repository 1: the short name");
    expect(alert).toHaveTextContent("Look-back days: use a whole number from 1 to 30.");
    expect(screen.getByLabelText("Short name of acme/platform")).toHaveAttribute("aria-invalid", "true");
    expect(days).toHaveAccessibleDescription(/use a whole number from 1 to 30/);
  });

  it("saves the whole form as the wire settings and says so", async () => {
    const { onSave } = renderCard();
    await userEvent.type(screen.getByLabelText("Short name of acme/legacy-tools"), "lt");
    await userEvent.type(screen.getByLabelText("Release branches of acme/docs-site"), "main, stable/*");
    await userEvent.click(screen.getByLabelText("Refresh my dashboard automatically every day"));
    await userEvent.selectOptions(screen.getByLabelText("Time of day"), "07:45");
    await userEvent.click(screen.getByRole("button", { name: "Save dashboard settings" }));
    const settings = saved(onSave);
    expect(settings.repositories[5]).toMatchObject({
      repo: { owner: "acme", name: "legacy-tools" },
      alias: "LT",
    });
    expect(settings.repositories[4]!.branch_patterns).toEqual(["main", "stable/*"]);
    expect(settings).toMatchObject({
      schedule_enabled: false,
      schedule_time: "07:45",
      time_zone: "Europe/Warsaw",
    });
    expect(await screen.findByRole("status")).toHaveTextContent("Dashboard settings saved.");
  });

  it("shows the server's refusal, which names the field", async () => {
    renderCard(VIEW, vi.fn().mockRejectedValue(new Error("Time zone: not a known time zone.")));
    await userEvent.click(screen.getByRole("button", { name: "Save dashboard settings" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Not saved: Time zone: not a known time zone.",
    );
  });

  it("adds and removes repository rows; a new row's policy must be complete", async () => {
    const { onSave } = renderCard({ ...VIEW, settings: VIEW.defaults });
    expect(screen.getByText(/No repositories yet/)).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Add repository" }));
    await userEvent.type(screen.getByLabelText("Repository 1"), "acme/web");
    await userEvent.type(screen.getByLabelText("Policy path of acme/web"), "owned.json");
    await userEvent.click(screen.getByRole("button", { name: "Save dashboard settings" }));
    expect(onSave).not.toHaveBeenCalled();
    expect(screen.getByLabelText("Policy repository of acme/web")).toHaveAccessibleDescription(
      /both the policy repository and its path, or neither/,
    );
    expect(screen.getByRole("list", { name: "MemCan projects" })).toHaveTextContent("web");
    await userEvent.click(screen.getByRole("button", { name: "Remove acme/web" }));
    await userEvent.click(screen.getByRole("button", { name: "Save dashboard settings" }));
    expect(saved(onSave).repositories).toEqual([]);
  });

  it("Reset to defaults fills the form but keeps the time zone, and sends nothing", async () => {
    const { onSave } = renderCard();
    await userEvent.click(screen.getByRole("button", { name: "Reset to defaults" }));
    expect(onSave).not.toHaveBeenCalled();
    expect(screen.getByRole("status")).toHaveTextContent("Defaults filled in. Save to keep them.");
    expect(screen.queryByRole("table", { name: "Repositories" })).toBeNull();
    expect(screen.getByLabelText("Time zone")).toHaveValue("Europe/Warsaw");
    await userEvent.click(screen.getByRole("button", { name: "Save dashboard settings" }));
    expect(saved(onSave)).toEqual({ ...VIEW.defaults, time_zone: "Europe/Warsaw" });
  });

  it("without MemCan the TODO switch is off and says why", () => {
    renderCard({ ...VIEW, memcan_configured: false });
    const toggle = screen.getByLabelText("Show my MemCan TODOs");
    expect(toggle).toBeDisabled();
    expect(toggle).not.toBeChecked();
    expect(toggle).toHaveAccessibleDescription(/MemCan is not configured on this server/);
  });

  it("without a stored time zone says the automatic refresh does not run yet", () => {
    renderCard({ ...VIEW, settings: VIEW.defaults });
    const zone = screen.getByLabelText("Time zone");
    expect(zone).toHaveValue("");
    expect(zone).toHaveAccessibleDescription(/automatic refresh does not run/);
  });
});
