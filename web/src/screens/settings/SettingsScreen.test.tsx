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
    await userEvent.click(await within(repo).findByText(/Show the .* built-in rules/));
    expect(await within(repo).findByText("tests/**")).toBeVisible();
    expect(repo).toHaveTextContent("acme/node-sdk");
    expectTrustWording(document.body.textContent ?? "");
  });

  it("LLM not working: the plain message", async () => {
    setScenario("llm-error");
    renderRoute("/settings");
    const llm = await region("LLM status");
    expect(await within(llm).findByText("not working")).toBeVisible();
    expect(llm).toHaveTextContent("The LLM provider's usage limit was reached.");
  });

  it("installations not checked: said plainly, the rest still shown", async () => {
    setScenario("installations-not-checked");
    renderRoute("/settings");
    const app = await region("GitHub App");
    expect(app).toHaveTextContent("GitHub App installations: not checked. Signing in again may help.");
    expect(within(app).getByRole("link", { name: "Sign in with GitHub" })).toHaveAttribute(
      "href",
      "/auth/github/login",
    );
    expect(await region("LLM status")).toHaveTextContent("working");
  });

  it("repository settings not known yet: a neutral note, not an error", async () => {
    const notKnown =
      "Not known yet: the repository settings are read during an analysis and appear once one has run.";
    server.use(
      http.get("/api/v1/repos/:owner/:name/settings", () =>
        HttpResponse.json({ code: "analysis_not_finished", message: notKnown }, { status: 409 }),
      ),
    );
    renderRoute("/settings");
    const note = await screen.findByText(notKnown);
    expect(note).toHaveAttribute("role", "status");
    expect(screen.queryByRole("alert")).toBeNull();
    // The note sits in the repository's card, like the facts it stands in for.
    expect(within(await region(/^Repository acme\/node-client/)).getByText(notKnown)).toBe(note);
  });

  it("no LLM call yet, no profile, no related repositories: said, not hidden", async () => {
    setScenario("llm-unknown");
    renderRoute("/settings");
    expect(await region("LLM status")).toHaveTextContent("not known yet");
    expect(screen.getByLabelText(/written in/)).toHaveValue("pl");
    // The card shows "Loading…" first, then the facts.
    const repo = () => screen.getByRole("region", { name: /acme\/node-client/ });
    await waitFor(() => expect(repo()).toHaveTextContent("No project profile"));
    expect(repo()).toHaveTextContent("None detected");
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

  it("an unknown ?repo= is said, with the picker; nothing else is shown in its place and the URL stays", async () => {
    const router = renderRoute("/settings?repo=acme/unknown");
    expect(await screen.findByText("Repository acme/unknown not found in this installation.")).toBeVisible();
    expect(screen.getByRole("combobox", { name: "Repository" })).toHaveValue("");
    expect(screen.queryByRole("region", { name: /^Repository acme\/node-client/ })).toBeNull();
    expect(router.state.location.search).toBe("?repo=acme/unknown");
    await userEvent.click(screen.getByRole("combobox", { name: "Repository" }));
    await userEvent.click(await screen.findByRole("option", { name: "acme/node-client" }));
    expect(await region(/^Repository acme\/node-client/)).toBeVisible();
    expect(router.state.location.search).toBe("?repo=acme%2Fnode-client");
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
