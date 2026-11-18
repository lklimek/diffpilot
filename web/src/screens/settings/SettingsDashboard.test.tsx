import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import {
  resetDashboardSettings,
  setDashboardSettingsScenario,
} from "../../mocks/handlers/dashboard-settings";

startMockServer();
afterEach(resetDashboardSettings);

const region = (name: string) => screen.findByRole("region", { name });

describe("Settings: dashboard section, ranking effort, access tokens (plan §F.6)", () => {
  it("shows the dashboard settings, the ranking effort in the LLM card and the tokens", async () => {
    renderRoute("/settings");
    const dashboard = await region("Dashboard");
    expect(dashboard).toHaveTextContent("Your login alice is always scanned.");
    expect(within(dashboard).getByLabelText("Short name of acme/platform")).toHaveValue("PLAT");
    const llm = await region("LLM model");
    expect(within(llm).getByLabelText("Dashboard ranking effort")).toHaveValue("low");
    expect(await screen.findByText("dispatcher agent")).toBeVisible();
    expectTrustWording(document.body.textContent ?? "");
  });

  it("saves the form and keeps the saved values", async () => {
    renderRoute("/settings");
    const dashboard = await region("Dashboard");
    const days = within(dashboard).getByLabelText("Look-back days");
    await userEvent.clear(days);
    await userEvent.type(days, "14");
    await userEvent.click(within(dashboard).getByRole("button", { name: "Save dashboard settings" }));
    expect(await within(dashboard).findByRole("status")).toHaveTextContent("Dashboard settings saved.");
    expect(days).toHaveValue(14);
  });

  it("shows the server's refusal naming the field", async () => {
    setDashboardSettingsScenario("save-rejected");
    renderRoute("/settings");
    const dashboard = await region("Dashboard");
    await userEvent.click(within(dashboard).getByRole("button", { name: "Save dashboard settings" }));
    expect(await within(dashboard).findByRole("alert")).toHaveTextContent(
      'Not saved: Time zone: "Etc/Unknown" is not a known time zone.',
    );
  });

  it("saves the ranking effort and shows who changed it", async () => {
    renderRoute("/settings");
    const llm = await region("LLM model");
    await userEvent.selectOptions(within(llm).getByLabelText("Dashboard ranking effort"), "medium");
    expect(await screen.findByText("Ranking effort saved.")).toBeVisible();
    const card = await region("LLM model");
    expect(within(card).getByLabelText("Dashboard ranking effort")).toHaveValue("medium");
    expect(card).toHaveTextContent("changed by alice");
  });

  it("creates a token, shows its secret once, then lists it", async () => {
    renderRoute("/settings");
    await screen.findByText("dispatcher agent");
    const tokens = await region("Access tokens");
    await userEvent.click(within(tokens).getByRole("button", { name: "New token" }));
    await userEvent.type(screen.getByLabelText("Name"), "ci agent");
    await userEvent.click(screen.getByRole("button", { name: "Create token" }));
    const secret = await screen.findByLabelText("Your new token");
    expect((secret as HTMLInputElement).value).toMatch(/^srp_[a-z0-9]{8}_/);
    const value = (secret as HTMLInputElement).value;
    await userEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(await within(tokens).findByText("ci agent")).toBeVisible();
    expect(document.body.innerHTML).not.toContain(value);
    await userEvent.click(within(tokens).getByRole("button", { name: "Revoke ci agent" }));
    await userEvent.click(within(tokens).getByRole("button", { name: "Revoke token" }));
    await waitFor(() => expect(within(tokens).queryByText("ci agent")).toBeNull());
  });

  it("the server refuses an eleventh token", async () => {
    setDashboardSettingsScenario("token-limit");
    renderRoute("/settings");
    await screen.findByText("agent 10");
    const tokens = await region("Access tokens");
    expect(within(tokens).getByRole("button", { name: "New token" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    expect(tokens).toHaveTextContent("10 of 10 tokens in use.");
  });

  it("settings that cannot be read: plain errors in both cards", async () => {
    setDashboardSettingsScenario("load-fails");
    renderRoute("/settings");
    expect(await within(await region("Dashboard")).findByRole("alert")).toHaveTextContent(
      "cannot be read right now",
    );
    expect(await within(await region("Access tokens")).findByRole("alert")).toHaveTextContent(
      "cannot be read right now",
    );
  });

  it("server without the dashboard routes (dark launch): nothing of the dashboard is shown", async () => {
    setDashboardSettingsScenario("not-deployed");
    renderRoute("/settings");
    const llm = await region("LLM model");
    await waitFor(() => expect(screen.queryByText("Loading the dashboard settings…")).toBeNull());
    await waitFor(() => expect(screen.queryByText("Loading your access tokens…")).toBeNull());
    expect(screen.queryByRole("region", { name: "Dashboard" })).toBeNull();
    expect(screen.queryByRole("region", { name: "Access tokens" })).toBeNull();
    expect(within(llm).queryByLabelText("Dashboard ranking effort")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
