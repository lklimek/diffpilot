import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { AnalysisState, Flow, FlowAnalysisState, Tour } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import { seedFor } from "../../mocks/fixtures";
import { FlowsProgressMenu } from "./FlowsProgressMenu";

const tour = seedFor("default", "tour") as Tour;
const withStates = (states: FlowAnalysisState[]): Flow[] =>
  tour.flows.map((f, i) => ({
    ...f,
    ...(states[i] !== "analysed" ? { steps: [] } : {}),
    analysis_status: { state: states[i]!, started_at: null, message: null },
  }));
const running: AnalysisState = { state: "running", done: 2, total: 5 };

function setup(over: Partial<Parameters<typeof FlowsProgressMenu>[0]> = {}) {
  const props = {
    flows: withStates(["analysed", "analysed", "analysing", "queued", "not_analysed"]),
    analysis: running,
    ticked: tour.flows[0]!.steps.map((s) => s.id),
    background: true,
    onOpenFlow: vi.fn(),
    onRequest: vi.fn(),
    onPause: vi.fn(),
    onResume: vi.fn(),
    onBackground: vi.fn(),
    ...over,
  };
  render(<FlowsProgressMenu {...props} />);
  return props;
}

const button = () => screen.getByRole("button", { name: /^Your progress/ });

describe("FlowsProgressMenu (proposal §8)", () => {
  it("is the progress counter; it opens every flow with its analysis and review state", async () => {
    setup();
    expect(button()).toHaveAccessibleName(
      "Your progress 1 / 5: flows reviewed. Show the flows with their analysis and review state",
    );
    expect(button()).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(button());
    const list = screen.getByRole("region", { name: "Flows: analysis and review" });
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(5);
    expect(rows[0]).toHaveTextContent("☑ reviewed");
    expect(rows[0]).toHaveTextContent(
      `${tour.flows[0]!.steps.length} / ${tour.flows[0]!.steps.length} steps ticked`,
    );
    expect(rows[2]).toHaveTextContent("Being analysed");
    expect(rows[3]).toHaveTextContent("Queued");
    expect(rows[4]).toHaveTextContent("Not analysed yet");
    expect(rows[4]).toHaveTextContent("no steps yet");
    expect(list).toHaveTextContent("Analysing all remaining flows: flow 3 now, then flow 4.");
    expectTrustWording(list.textContent ?? "");
  });

  it("offers ▶ on a flow not analysed, ⏸ on the one being analysed, Stop for all remaining, never Cancel", async () => {
    const props = setup();
    await userEvent.click(button());
    const list = screen.getByRole("region", { name: "Flows: analysis and review" });
    await userEvent.click(within(list).getByRole("button", { name: "Analyse flow 5" }));
    expect(props.onRequest).toHaveBeenCalledWith(tour.flows[4]!.id);
    await userEvent.click(within(list).getByRole("button", { name: "Pause analysis" }));
    expect(props.onPause).toHaveBeenCalledOnce();
    await userEvent.click(within(list).getByRole("button", { name: "Stop" }));
    expect(props.onBackground).toHaveBeenCalledWith(false);
    expect(within(list).queryByRole("button", { name: /cancel/i })).toBeNull();
    await userEvent.click(within(list).getByRole("link", { name: /^1 / }));
    expect(props.onOpenFlow).toHaveBeenCalledWith(tour.flows[0]!.id);
  });

  it("when paused says so with Resume, and offers Analyse all remaining flows while that is off", async () => {
    const props = setup({
      analysis: {
        state: "paused",
        reason: "user",
        paused_at: "2026-11-18T14:02:00Z",
        resumes_at: null,
        done: 2,
        total: 5,
      },
      flows: withStates(["analysed", "analysed", "not_analysed", "not_analysed", "not_analysed"]),
      background: false,
    });
    await userEvent.click(button());
    const list = screen.getByRole("region", { name: "Flows: analysis and review" });
    expect(list).toHaveTextContent(/Analysis paused by you at .*\. Nothing starts until you resume\./);
    await userEvent.click(within(list).getByRole("button", { name: "▶ Resume" }));
    expect(props.onResume).toHaveBeenCalledOnce();
    await userEvent.click(within(list).getByRole("button", { name: "Analyse all remaining flows" }));
    expect(props.onBackground).toHaveBeenCalledWith(true);
  });

  it("Esc closes it and gives the focus back to the button", async () => {
    setup({
      flows: withStates(["analysed", "analysed", "analysed", "analysed", "analysed"]),
      analysis: { state: "finished" } as AnalysisState,
    });
    await userEvent.click(button());
    expect(screen.getByRole("region", { name: "Flows: analysis and review" })).toHaveTextContent(
      "All flows are analysed.",
    );
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("region", { name: "Flows: analysis and review" })).toBeNull();
    expect(button()).toHaveFocus();
  });

  it("offers ▶ again on a failed flow, also in a finished analysis; only a click starts it", async () => {
    const onRetry = vi.fn();
    setup({
      flows: withStates(["analysed", "analysed", "analysed", "analysed", "failed"]),
      analysis: { state: "finished", total: 5 },
      onRequest: undefined,
      onRetry,
    });
    await userEvent.click(button());
    const row = within(screen.getByRole("region", { name: "Flows: analysis and review" })).getAllByRole(
      "listitem",
    )[4]!;
    expect(onRetry).not.toHaveBeenCalled();
    await userEvent.click(within(row).getByRole("button", { name: "Analyse flow 5 again" }));
    expect(onRetry).toHaveBeenCalledWith(tour.flows[4]!.id);
  });
});
