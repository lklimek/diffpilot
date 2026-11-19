import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { expectTrustWording } from "../../__tests__/wording";
import type { AnalysisState, AnalysisSummary } from "../../api/generated/types";
import { seedFor } from "../../mocks/fixtures";
import { liveAnalysis } from "../../mocks/live-analysis";
import { setScenario } from "../../mocks/scenarios";

const server = startMockServer();
const analysis = seedFor("default", "analysis") as AnalysisSummary;
const ID = analysis.id;

beforeEach(() => {
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
});
afterEach(() => vi.unstubAllGlobals());

/** Opens the progress screen and delivers the snapshot `state`. */
async function showAt(state: AnalysisState) {
  const router = renderRoute(`/analyses/${ID}/progress`);
  await waitFor(() => expect(FakeEventSource.instances).toHaveLength(1));
  emit(state);
  return router;
}

const emit = (state: AnalysisState) =>
  act(() => FakeEventSource.latest().emit({ event: "analysis", summary: { ...analysis, state } }));

const pageText = () => document.body.textContent ?? "";

describe("Analysis progress (brief §4.2, sse-events.md)", () => {
  it("before the snapshot: connecting", async () => {
    renderRoute(`/analyses/${ID}/progress`);
    expect(await screen.findByRole("status")).toHaveTextContent("Connecting…");
    expect(FakeEventSource.latest().url).toBe(`/api/v1/analyses/${ID}/events`);
  });

  it("planning → running k of n → finished with Open; the PR is in the top bar", async () => {
    await showAt({ state: "planning" });
    expect(screen.getByText("Planning flows…")).toBeVisible();
    expect(screen.getByRole("banner")).toHaveTextContent("#482");
    expect(screen.queryByRole("link", { name: "Open" })).toBeNull();
    emit({ state: "running", done: 3, total: 5 });
    expect(screen.getByText("3 of 5 flows analysed")).toBeVisible();
    expect(screen.queryByRole("link", { name: "Open" })).toBeNull();
    emit({ state: "finished", total: 5 });
    expect(screen.getByText("5 of 5 flows analysed")).toBeVisible();
    expect(screen.getByRole("link", { name: "Open" })).toHaveAttribute("href", `/analyses/${ID}`);
    expect(screen.queryByRole("button", { name: "Cancel analysis" })).toBeNull();
    act(() =>
      FakeEventSource.latest().emit({
        event: "analysis",
        summary: { ...analysis, state: { state: "finished", total: 5 }, has_my_state: true },
      }),
    );
    expect(screen.getByRole("link", { name: "Open" })).toHaveAttribute("href", `/analyses/${ID}`);
    expectTrustWording(pageText());
  });

  it("paused: usage limit and resume time", async () => {
    const resumes = new Date();
    resumes.setHours(23, 58, 0, 0);
    await showAt({ state: "paused", resumes_at: resumes.toISOString(), done: 0, total: null });
    expect(screen.getByText("Paused until 23:58")).toBeVisible();
  });

  it("failed before any flow: plain description; Retry asks the server to retry", async () => {
    setScenario("failed-early");
    renderRoute(`/analyses/${ID}/progress`);
    await waitFor(() => expect(FakeEventSource.instances).toHaveLength(1));
    // The snapshot's stage data says planning had not finished.
    act(() => FakeEventSource.latest().emit({ event: "analysis", summary: liveAnalysis() }));
    expect(screen.getByText("The analysis failed before any flow was planned")).toBeVisible();
    expect(screen.getByText("The repository could not be fetched. Retry the analysis.")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(liveAnalysis().state).toEqual({ state: "planning" }));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("failed in a flow names the flow", async () => {
    await showAt({
      state: "failed",
      flow: { id: "f3", title: "Classify RPC errors" },
      message: "Bad output.",
    });
    expect(screen.getByText("Analysis of flow “Classify RPC errors” failed")).toBeVisible();
    expect(screen.getByText("Bad output.")).toBeVisible();
  });

  it("Cancel analysis returns to the review start page with 'Analysis cancelled. Check the PR to start again.'", async () => {
    const router = await showAt({ state: "running", done: 1, total: 5 });
    await userEvent.click(screen.getByRole("button", { name: "Cancel analysis" }));
    await userEvent.click(screen.getByRole("button", { name: "Confirm cancel" }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/review"));
    expect(liveAnalysis().state).toEqual({ state: "cancelled" });
    expect(await screen.findByText("Analysis cancelled. Check the PR to start again.")).toBeVisible();
    expect(screen.getByLabelText("GitHub pull request URL")).toHaveValue(analysis.pr.html_url);
  });

  it("cancelled elsewhere (event): returns to Home as well", async () => {
    const router = await showAt({ state: "running", done: 1, total: 5 });
    emit({ state: "cancelled" });
    await waitFor(() => expect(router.state.location.pathname).toBe("/review"));
  });

  it("a failed cancel request is reported and the analysis stays", async () => {
    server.use(
      http.post(`/api/v1/analyses/${ID}/cancel`, () =>
        HttpResponse.json(
          { code: "internal", message: "Something went wrong on the server." },
          { status: 500 },
        ),
      ),
    );
    const router = await showAt({ state: "running", done: 1, total: 5 });
    await userEvent.click(screen.getByRole("button", { name: "Cancel analysis" }));
    await userEvent.click(screen.getByRole("button", { name: "Confirm cancel" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Something went wrong on the server.");
    expect(router.state.location.pathname).toBe(`/analyses/${ID}/progress`);
  });

  it("connection lost: keeps the last state and says it reconnects; the next snapshot clears it", async () => {
    await showAt({ state: "running", done: 2, total: 5 });
    act(() => FakeEventSource.latest().drop());
    expect(screen.getByText("Connection lost; reconnecting…")).toBeVisible();
    expect(screen.getByText("2 of 5 flows analysed")).toBeVisible();
    act(() => FakeEventSource.latest().open());
    emit({ state: "running", done: 4, total: 5 });
    expect(screen.queryByText("Connection lost; reconnecting…")).toBeNull();
    expect(screen.getByText("4 of 5 flows analysed")).toBeVisible();
  });

  it("unknown analysis: plain error and a way back", async () => {
    renderRoute("/analyses/00000000-0000-0000-0000-000000000000/progress");
    await waitFor(() => expect(FakeEventSource.instances).toHaveLength(1));
    act(() => FakeEventSource.latest().fail());
    expect(await screen.findByRole("alert")).toHaveTextContent("This analysis does not exist.");
    expect(screen.getByRole("link", { name: "My work" })).toHaveAttribute("href", "/");
  });
});
