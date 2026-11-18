// Tour screen: analysis state, drafts, background refresh failures and the user's questions.
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import type { AnalysisSummary } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { setScenario } from "../../mocks/scenarios";

startMockServer();
const ID = (seedFor("default", "analysis") as AnalysisSummary).id;

beforeEach(() => {
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
});
afterEach(() => {
  vi.unstubAllGlobals();
  window.localStorage.clear();
  window.sessionStorage.clear();
});

const user = () => userEvent.setup();
const main = () => screen.getByRole("main");
const nav = () => screen.getByRole("navigation", { name: "Flows and steps" });
const stateBanner = () => within(main()).findByRole("region", { name: "Analysis state" });

async function open(scenario: Parameters<typeof setScenario>[0], hash = "") {
  setScenario(scenario);
  renderRoute(`/analyses/${ID}${hash}`);
  await within(await screen.findByRole("navigation", { name: "Flows and steps" })).findByRole("button", {
    name: /^Flow 1:/,
  });
}

describe("Tour: state of the analysis (paused, usage limit, failed)", () => {
  it("user pause: one line with who paused it and Resume; pending flows say the analysis is paused", async () => {
    await open("lazy-paused");
    expect(await stateBanner()).toHaveTextContent("Analysis paused by alice");
    expect(within(nav()).getAllByText("Not analysed yet — analysis paused").length).toBeGreaterThan(0);
    expect(within(nav()).queryByText("Queued")).toBeNull();
    expectTrustWording(document.body.textContent ?? "");
    await user().click(within(await stateBanner()).getByRole("button", { name: "Resume" }));
    await waitFor(() => expect(within(main()).queryByRole("region", { name: "Analysis state" })).toBeNull());
  });

  it("usage limit: the reason and the time it resumes, no Resume", async () => {
    await open("lazy-limit");
    const banner = await stateBanner();
    expect(banner).toHaveTextContent("The LLM provider's usage limit was reached.");
    expect(banner).toHaveTextContent(/It resumes at .*14:20|It resumes at \d\d:\d\d/);
    expect(within(banner).queryByRole("button", { name: "Resume" })).toBeNull();
  });

  it("failed: the failure sentence and Retry; pending flows never read Queued", async () => {
    await open("lazy-failed");
    const banner = await stateBanner();
    expect(banner).toHaveTextContent("The analysis failed. The repository mirror could not be updated.");
    expect(within(nav()).getAllByText("Not analysed — the analysis failed").length).toBeGreaterThan(0);
    await user().click(within(nav()).getByRole("button", { name: /^Flow 2:/ }));
    expect(main()).toHaveTextContent("Not analysed — the analysis failed");
    expect(main()).not.toHaveTextContent(/\bQueued\b/);
    await user().click(within(banner).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(within(main()).queryByRole("region", { name: "Analysis state" })).toBeNull());
  });

  it("a failed analysis opened at a resumed step still shows its failure and Retry (Home's Show details and retry)", async () => {
    await open("lazy-failed", "#f1.1");
    const banner = await stateBanner();
    expect(within(banner).getByRole("button", { name: "Retry" })).toBeVisible();
  });
});
