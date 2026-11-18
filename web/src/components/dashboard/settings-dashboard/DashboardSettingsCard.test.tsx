import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { DashboardSettings, DashboardSettingsView } from "../../../api/generated/types";
import viewJson from "../../../../../fixtures/dashboard/api/settings.json";
import { DashboardSettingsCard } from "./DashboardSettingsCard";

const VIEW = viewJson as unknown as DashboardSettingsView;

function renderCard(
  view: DashboardSettingsView = VIEW,
  onSave = vi.fn().mockResolvedValue(view),
  onDirtyChange = vi.fn(),
) {
  render(<DashboardSettingsCard view={view} onSave={onSave} onDirtyChange={onDirtyChange} />);
  return { onSave, onDirtyChange, card: screen.getByRole("region", { name: "Dashboard" }) };
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
      "empty = main, master, release/*, hotfix/*, vX.Y, vX.Y-dev",
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

  it("explains the ownership policy: column help, placeholders and a visible hint under the fields", () => {
    const { card } = renderCard();
    const table = within(card).getByRole("table", { name: "Repositories" });
    const help = within(table).getByRole("link", { name: "Help: Ownership policy" });
    expect(help).toHaveAccessibleDescription(
      /What: Optional\. A JSON file listing the paths you own in this repository/,
    );
    expect(help).toHaveAccessibleDescription(
      /Example: Repository dashpay\/stale_prs_are_bad, path policies\/platform\.json\./,
    );
    const repo = screen.getByLabelText("Policy repository of acme/consensus");
    const path = screen.getByLabelText("Policy path of acme/consensus");
    expect(repo).toHaveAttribute("placeholder", "owner/name");
    expect(path).toHaveAttribute("placeholder", "policies/<repo>.json");
    expect(repo).toHaveAccessibleDescription("Optional — leave empty if there is no policy file");
    expect(path).toHaveAccessibleDescription("Optional — leave empty if there is no policy file");
    expect(within(table).getAllByText("Optional — leave empty if there is no policy file")[0]).toBeVisible();
  });

  it("release branches: built-ins shown for an empty field, X.Y and * explained, a list replaces the built-ins", () => {
    renderCard();
    const empty = screen.getByLabelText("Release branches of acme/platform");
    expect(empty).toHaveValue("");
    expect(empty).toHaveAttribute("placeholder", "empty = main, master, release/*, hotfix/*, vX.Y, vX.Y-dev");
    expect(empty).toHaveAccessibleDescription(
      "Leave empty for the built-ins. X.Y stands for a version (v1.8-dev matches vX.Y-dev); * matches any text. Your list replaces the built-ins.",
    );
    const row = empty.closest("td")!;
    expect(
      within(row).getByText("Built-ins: main, master, release/*, hotfix/*, vX.Y, vX.Y-dev"),
    ).toBeVisible();
    const set = screen.getByLabelText("Release branches of acme/consensus");
    expect(within(set.closest("td")!).queryByText(/^Built-ins:/)).toBeNull();
    expect(
      screen.getByText(
        "Leave empty for the built-ins. X.Y stands for a version (v1.8-dev matches vX.Y-dev); * matches any text. Your list replaces the built-ins.",
      ),
    ).toBeVisible();
  });

  it("Excluded authors and Known bots: two lists with their defaults and what each does", async () => {
    const { onSave } = renderCard();
    expect(screen.getByRole("list", { name: "Excluded authors (never shown)" })).toHaveTextContent(
      "dependabot[bot]",
    );
    const bots = screen.getByRole("list", { name: "Known bots" });
    for (const login of ["thepastaclaw", "Claudius-Maginificent", "DCG-Claude"])
      expect(bots).toHaveTextContent(login);
    expect(
      screen.getByText(
        "Pull requests and issues written by these accounts are never shown in Needs you, also when a review is requested from you or they are assigned to you.",
      ),
    ).toBeVisible();
    expect(
      screen.getByText(
        "Treated as bots: their review requests and comments do not count as a person waiting for you. GitHub Apps (…[bot]) are always bots.",
      ),
    ).toBeVisible();
    await userEvent.type(screen.getByLabelText("Add a known bot"), "renovate-helper");
    await userEvent.click(screen.getByRole("button", { name: "Add to known bots" }));
    await userEvent.click(screen.getByRole("button", { name: "Save dashboard settings" }));
    expect(saved(onSave).known_bots).toEqual([
      "thepastaclaw",
      "Claudius-Maginificent",
      "DCG-Claude",
      "renovate-helper",
    ]);
    expect(saved(onSave).skip_authors).toEqual(VIEW.settings.skip_authors);
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
      /the policy repository is missing\. Give the repository and the path of the policy file, or leave both empty\./,
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

  it("says when the form differs from the stored settings and reports it", async () => {
    const onSave = vi.fn();
    const { onDirtyChange, card } = renderCard(VIEW, onSave);
    expect(card).not.toHaveTextContent("Unsaved changes");
    expect(onDirtyChange).toHaveBeenLastCalledWith(false);
    await userEvent.click(screen.getByLabelText("Watch issues of acme/node-client"));
    expect(card).toHaveTextContent("Unsaved changes");
    expect(onDirtyChange).toHaveBeenLastCalledWith(true);
    await userEvent.click(screen.getByLabelText("Watch issues of acme/node-client"));
    expect(card).not.toHaveTextContent("Unsaved changes");
    expect(onDirtyChange).toHaveBeenLastCalledWith(false);
  });

  it("offers a stored time zone the browser does not list", () => {
    renderCard({ ...VIEW, settings: { ...VIEW.settings, time_zone: "Asia/Calcutta-Old" } });
    expect(screen.getByLabelText("Time zone")).toHaveValue("Asia/Calcutta-Old");
  });

  it("without a stored time zone says the automatic refresh does not run yet", () => {
    renderCard({ ...VIEW, settings: VIEW.defaults });
    const zone = screen.getByLabelText("Time zone");
    expect(zone).toHaveValue("");
    expect(zone).toHaveAccessibleDescription(/automatic refresh does not run/);
  });
});
