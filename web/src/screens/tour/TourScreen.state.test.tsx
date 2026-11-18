// Tour screen: analysis state, drafts, background refresh failures and the user's questions.
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HttpResponse, http } from "msw";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import type { AnalysisSummary } from "../../api/generated/types";
import { seedFor, setFixtureState } from "../../mocks/fixtures";
import { setScenario } from "../../mocks/scenarios";

const server = startMockServer();
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

describe("Tour: unsent comment drafts survive navigation", () => {
  const inspector = () => screen.getByRole("complementary", { name: "Inspector" });
  const commentOn95 = async () => {
    const code = await within(main()).findByRole("region", { name: /lines 88 to 96/ });
    await user().click(within(code).getByRole("button", { name: "Comment on line 95" }));
    return within(inspector()).getByRole("textbox", { name: "Comment on src/rpc/client.rs:95" });
  };

  it("Next and back to the line restore the text; a reload of the tab too", async () => {
    await open(null, "#f1.2");
    await user().type(await commentOn95(), "Is the deadline respected?");
    await user().click(within(main()).getByRole("button", { name: /^Next/ }));
    expect(within(inspector()).queryByRole("textbox", { name: /^Comment on/ })).toBeNull();
    await user().click(within(main()).getByRole("button", { name: /^← Previous/ }));
    expect(await commentOn95()).toHaveValue("Is the deadline respected?");
    expect(within(inspector()).getByText("Draft")).toBeVisible();
    expect(JSON.stringify(window.sessionStorage)).toContain("Is the deadline respected?");
  });
});

describe("Tour: the user's questions", () => {
  it("steps with questions are marked in the menu; one entry opens every question with its answer and step", async () => {
    setFixtureState("resume");
    await open(null, "#f1.5");
    const marker = await within(nav()).findByText("?1");
    expect(marker).toHaveAccessibleDescription("1 question asked on this step");
    await user().click(within(nav()).getByRole("button", { name: "Your questions and answers: 1" }));
    const page = await within(main()).findByRole("article", { name: "Your questions and answers (1)" });
    expect(page).toHaveTextContent("Does the node deduplicate broadcast_tx by transaction id?");
    expect(page).toHaveTextContent("LLM answer");
    await user().click(within(page).getByRole("button", { name: /^Flow 1 · step 5/ }));
    expect(within(main()).queryByRole("article", { name: /Your questions/ })).toBeNull();
    expect(within(nav()).getByRole("button", { name: /^Step 5: .*1 question asked/ })).toHaveAttribute(
      "aria-current",
      "step",
    );
  });
});

describe("Tour: a failed background refresh keeps the open review", () => {
  it("keeps the last good data and a typed draft; says it could not refresh, with Retry", async () => {
    await open(null, "#f1.2");
    const inspector = () => screen.getByRole("complementary", { name: "Inspector" });
    const code = await within(main()).findByRole("region", { name: /lines 88 to 96/ });
    await user().click(within(code).getByRole("button", { name: "Comment on line 95" }));
    await user().type(within(inspector()).getByRole("textbox", { name: /^Comment on/ }), "half written");
    server.use(
      http.get("/api/v1/analyses/:id/tour", () =>
        HttpResponse.json({ code: "internal", message: "Tour could not be built." }, { status: 500 }),
      ),
    );
    const summary = seedFor("default", "analysis") as AnalysisSummary;
    act(() =>
      FakeEventSource.latest().emit({ event: "analysis", summary: { ...summary, review_ready: true } }),
    );
    const alert = await within(main()).findByRole("alert");
    expect(alert).toHaveTextContent("Could not refresh the review: Tour could not be built.");
    expect(within(inspector()).getByRole("textbox", { name: /^Comment on/ })).toHaveValue("half written");
    expect(nav()).toBeVisible();
    server.resetHandlers();
    await user().click(within(alert).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(within(main()).queryByText(/Could not refresh the review/)).toBeNull());
  });
});
