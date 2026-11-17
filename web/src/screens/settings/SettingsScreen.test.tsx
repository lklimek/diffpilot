import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { describe, expect, it } from "vitest";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import { SETTINGS, setScenario } from "../../mocks/scenarios";

const server = startMockServer();
const region = (name: string | RegExp) => screen.findByRole("region", { name });

describe("Settings (brief §4.4)", () => {
  it("default: LLM working with the time of the last call, language, App, repository rules", async () => {
    renderRoute("/settings");
    expect(await screen.findByRole("heading", { level: 1, name: "Settings" })).toBeVisible();
    const llm = await region("LLM status");
    expect(within(llm).getByText("working")).toBeVisible();
    expect(llm).toHaveTextContent("last LLM call:");
    expect(screen.getByLabelText(/written in/)).toHaveValue("en");
    expect(await region("GitHub App")).toHaveTextContent("acme: selected repositories");
    const repo = await region(/acme\/node-client/);
    expect(await within(repo).findByText("tests/**")).toBeVisible();
    expect(repo).toHaveTextContent("acme/node-sdk");
    expectTrustWording(document.body.textContent ?? "");
  });

  it("LLM not working: the plain message", async () => {
    setScenario("llm-error");
    renderRoute("/settings");
    const llm = await region("LLM status");
    expect(await within(llm).findByText("not working")).toBeVisible();
    expect(llm).toHaveTextContent("Codex usage limit reached. It resets at 14:20.");
  });

  it("no LLM call yet, no profile, no related repositories: said, not hidden", async () => {
    setScenario("llm-unknown");
    renderRoute("/settings");
    expect(await region("LLM status")).toHaveTextContent("not known yet");
    expect(screen.getByLabelText(/written in/)).toHaveValue("pl");
    const repo = await region(/acme\/node-client/);
    await waitFor(() => expect(repo).toHaveTextContent("No project profile"));
    expect(repo).toHaveTextContent("None detected");
  });

  it("changing the language saves it", async () => {
    renderRoute("/settings");
    await region("LLM status");
    await userEvent.selectOptions(screen.getByLabelText(/written in/), "pl");
    await waitFor(() => expect(screen.getByLabelText(/written in/)).toHaveValue("pl"));
    await waitFor(() => expect(screen.getByLabelText(/written in/)).toBeEnabled());
  });

  it("no repository analysed yet", async () => {
    server.use(http.get("/api/v1/settings", () => HttpResponse.json({ ...SETTINGS, repos: [] })));
    renderRoute("/settings");
    expect(
      await screen.findByText("No pull request has been analysed yet, so no repository settings exist."),
    ).toBeVisible();
  });

  it("settings cannot be read: plain error", async () => {
    server.use(
      http.get("/api/v1/settings", () =>
        HttpResponse.json(
          { code: "internal", message: "Something went wrong on the server." },
          { status: 500 },
        ),
      ),
    );
    renderRoute("/settings");
    expect(await screen.findByRole("alert")).toHaveTextContent("Something went wrong on the server.");
  });

  it("signed out: the sign-in card instead of settings", async () => {
    setScenario("signed-out");
    renderRoute("/settings");
    expect(await screen.findByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
    expect(screen.queryByRole("region", { name: "LLM status" })).toBeNull();
  });
});
