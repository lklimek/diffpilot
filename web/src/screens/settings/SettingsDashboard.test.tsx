import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { afterEach, describe, expect, it } from "vitest";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import {
  resetDashboardSettings,
  setDashboardSettingsScenario,
} from "../../mocks/handlers/dashboard-settings";

const server = startMockServer();
afterEach(resetDashboardSettings);

// A card once its data has loaded or failed (a 503 GET is retried once after 1 s by the API client).
const region = async (name: string) => {
  await waitFor(() => expect(screen.getByRole("region", { name })).not.toHaveTextContent(/Loading/), {
    timeout: 3000,
  });
  return screen.getByRole("region", { name });
};

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

  it("a bare 404 of the dashboard routes is an error, never a reason to hide the dashboard part", async () => {
    server.use(
      http.get("/api/v1/settings/dashboard", () => new HttpResponse(null, { status: 404 })),
      http.get("/api/v1/settings/access-tokens", () => new HttpResponse(null, { status: 404 })),
    );
    renderRoute("/settings");
    const llm = await region("LLM model");
    expect(await within(await region("Dashboard")).findByRole("alert")).toBeVisible();
    expect(await within(await region("Access tokens")).findByRole("alert")).toBeVisible();
    // The ranking effort belongs to the installation's LLM settings, whatever the dashboard settings answer.
    expect(within(llm).getByLabelText("Dashboard ranking effort")).toBeVisible();
  });

  it("saving the ranking effort keeps an unsaved model choice and the focus", async () => {
    renderRoute("/settings");
    const llm = await region("LLM model");
    await userEvent.selectOptions(within(llm).getByLabelText("Model", { exact: true }), "gpt-6-astra");
    const effort = within(llm).getByLabelText("Dashboard ranking effort");
    await userEvent.selectOptions(effort, "medium");
    expect(await screen.findByText("Ranking effort saved.")).toBeVisible();
    const card = await region("LLM model");
    expect(within(card).getByLabelText("Model", { exact: true })).toHaveValue("gpt-6-astra");
    expect(within(card).getByLabelText("Dashboard ranking effort")).toHaveFocus();
  });

  it("unsaved dashboard settings: leaving asks first, Stay keeps them, Leave goes; the page unload is guarded", async () => {
    const router = renderRoute("/settings");
    const dashboard = await region("Dashboard");
    await userEvent.click(within(dashboard).getByLabelText("Watch issues of acme/node-client"));
    expect(dashboard).toHaveTextContent("Unsaved changes");
    const unload = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(unload);
    expect(unload.defaultPrevented).toBe(true);
    // Another repository in the URL (same screen) is not leaving.
    await router.navigate("/settings?repo=acme%2Fnode-client");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    void router.navigate("/");
    const ask = await screen.findByRole("alertdialog", { name: "Unsaved dashboard settings" });
    expect(router.state.location.pathname).toBe("/settings");
    await userEvent.click(within(ask).getByRole("button", { name: "Stay" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(
      within(await region("Dashboard")).getByLabelText("Watch issues of acme/node-client"),
    ).toBeChecked();
    void router.navigate("/");
    await userEvent.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Leave without saving" }),
    );
    await waitFor(() => expect(router.state.location.pathname).toBe("/"));
  });

  it("after Save nothing is guarded", async () => {
    renderRoute("/settings");
    const dashboard = await region("Dashboard");
    await userEvent.click(within(dashboard).getByLabelText("Watch issues of acme/node-client"));
    await userEvent.click(within(dashboard).getByRole("button", { name: "Save dashboard settings" }));
    await waitFor(() => expect(dashboard).not.toHaveTextContent("Unsaved changes"));
    await waitFor(() => {
      const unload = new Event("beforeunload", { cancelable: true });
      window.dispatchEvent(unload);
      expect(unload.defaultPrevented).toBe(false);
    });
  });

  it("Sign out with unsaved dashboard settings leaves without asking", async () => {
    const router = renderRoute("/settings");
    const dashboard = await region("Dashboard");
    await userEvent.click(within(dashboard).getByLabelText("Watch issues of acme/node-client"));
    await userEvent.click(screen.getByRole("button", { name: "Sign out" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/"));
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("a 404 with the API's error body is an error, not a missing route", async () => {
    server.use(
      http.get("/api/v1/settings/dashboard", () =>
        HttpResponse.json({ code: "not_found", message: "No dashboard settings here." }, { status: 404 }),
      ),
    );
    renderRoute("/settings");
    expect(await within(await region("Dashboard")).findByRole("alert")).toHaveTextContent(
      "No dashboard settings here.",
    );
  });
});
