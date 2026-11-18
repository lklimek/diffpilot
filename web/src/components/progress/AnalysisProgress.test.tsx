import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { expectTrustWording } from "../../__tests__/wording";
import type {
  AnalysisProgress as RunProgress,
  AnalysisState,
  RunStageState,
} from "../../api/generated/types";
import { AnalysisProgress, SILENT_MS, type ProgressActivity } from "./AnalysisProgress";

function show(state: AnalysisState, busy = false, activity?: ProgressActivity) {
  const onCancel = vi.fn();
  const onRetry = vi.fn();
  const view = render(
    <AnalysisProgress
      state={state}
      reviewHref="/analyses/a1"
      busy={busy}
      onCancel={onCancel}
      onRetry={onRetry}
      activity={activity}
    />,
  );
  return { onCancel, onRetry, ...view };
}

const NOW = Date.parse("2026-11-17T10:00:00Z");
const MIN = 60_000;
/** Connected, last server signal `signalAgo` ms ago, browser clock `offset` ms ahead of the server's. */
const activity = (over: Partial<ProgressActivity> = {}): ProgressActivity => ({
  now: NOW,
  clockOffset: 0,
  lastSignalAt: NOW - 5000,
  connected: true,
  ...over,
});
/** Started on the server `ago` ms before NOW (server clock). */
const startedAgo = (ago: number) => new Date(NOW - ago).toISOString();
const indicator = () => document.querySelector("[data-activity]");

const bar = () => screen.getByRole("progressbar", { name: "Analysis progress" });
/** Waiting facts of a run that failed with planning and PR context in these states. */
const failedAt = (planning: RunStageState, prContext: RunStageState): RunProgress => ({
  started_at: "2026-11-17T09:00:00Z",
  planning: { done: planning === "done" ? 1 : 0, total: planning === "done" ? 1 : null, state: planning },
  pr_context: { done: prContext === "done" ? 1 : 0, total: 1, state: prContext },
  flows: { done: 0, total: planning === "done" ? 5 : null, state: "pending" },
  finishing: { done: 0, total: 1, state: "pending" },
  estimate: null,
  percent: null,
});
const buttons = () => screen.queryAllByRole("button").map((b) => b.textContent);

describe("AnalysisProgress (brief §4.2)", () => {
  it("planning: indeterminate bar, 'Planning flows…', Cancel analysis", () => {
    show({ state: "planning" });
    expect(screen.getByText("Planning flows…")).toBeVisible();
    expect(bar()).not.toHaveAttribute("aria-valuenow");
    expect(bar()).toHaveAttribute("aria-valuetext", "Planning flows");
    expect(buttons()).toEqual(["Cancel analysis"]);
    expect(screen.queryByRole("link", { name: /^(Resume|Open) review$/ })).toBeNull();
  });

  it("running: k of n flows analysed, determinate bar, Cancel analysis", () => {
    show({ state: "running", done: 2, total: 5 });
    expect(screen.getByText("2 of 5 flows analysed")).toBeVisible();
    expect(bar()).toHaveAttribute("aria-valuenow", "2");
    expect(bar()).toHaveAttribute("aria-valuemax", "5");
    const [filled, empty] = Array.from(bar().children) as HTMLElement[];
    expect([filled!.style.flexGrow, empty!.style.flexGrow]).toEqual(["2", "3"]);
    expect(buttons()).toEqual(["Cancel analysis"]);
    expect(screen.queryByRole("link", { name: /^(Resume|Open) review$/ })).toBeNull();
  });

  it("paused: usage limit with the resume time", () => {
    const resumes = new Date();
    resumes.setHours(23, 59, 0, 0);
    show({ state: "paused", resumes_at: resumes.toISOString(), done: 2, total: 5 });
    expect(screen.getByText("Paused until 23:59 — 2 of 5")).toBeVisible();
    expect(screen.getByText("The LLM provider's usage limit was reached.")).toBeVisible();
    expect(bar()).toHaveAttribute("aria-valuenow", "2");
    expect(bar()).toHaveAttribute("aria-valuemax", "5");
    const [filled, empty] = Array.from(bar().children) as HTMLElement[];
    expect([filled!.style.flexGrow, empty!.style.flexGrow]).toEqual(["2", "3"]);
    expect(buttons()).toEqual(["Cancel analysis"]);
  });

  it("paused while planning with a resume time: no count", () => {
    const resumes = new Date();
    resumes.setHours(14, 20, 0, 0);
    show({ state: "paused", resumes_at: resumes.toISOString(), done: 0, total: null });
    expect(screen.getByText("Paused until 14:20")).toBeVisible();
  });

  it("paused while planning: no count, indeterminate bar", () => {
    show({ state: "paused", resumes_at: null, done: 0, total: null });
    expect(screen.getByText("Paused; the resume time is not known")).toBeVisible();
    expect(bar()).not.toHaveAttribute("aria-valuenow");
  });

  it("failed in a flow: names the flow, shows the plain description, Retry and Cancel", async () => {
    const { onRetry, onCancel } = show({
      state: "failed",
      flow: { id: "f3", title: "Classify RPC errors" },
      message: "The LLM answer did not match the expected format.",
    });
    expect(screen.getByText("Analysis of flow “Classify RPC errors” failed")).toBeVisible();
    expect(screen.getByText("The LLM answer did not match the expected format.")).toBeVisible();
    expect(buttons()).toEqual(["Cancel analysis", "Retry"]);
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledOnce();
    await userEvent.click(screen.getByRole("button", { name: "Cancel analysis" }));
    await userEvent.click(screen.getByRole("button", { name: "Confirm cancel" }));
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("failed before any flow: plain description and Retry", () => {
    render(
      <AnalysisProgress
        state={{
          state: "failed",
          flow: null,
          message: "The repository could not be fetched. Retry the analysis.",
        }}
        progress={failedAt("pending", "pending")}
        reviewHref="/analyses/a1"
        busy={false}
        onCancel={() => {}}
        onRetry={() => {}}
      />,
    );
    expect(screen.getByText("The analysis failed before any flow was planned")).toBeVisible();
    expect(screen.getByText("The repository could not be fetched. Retry the analysis.")).toBeVisible();
    expect(screen.getByRole("button", { name: "Retry" })).toBeEnabled();
  });

  it("finished: n of n flows analysed and Open review for a review not started yet", () => {
    show({ state: "finished", total: 5 });
    expect(screen.getByText("5 of 5 flows analysed")).toBeVisible();
    expect(bar()).toHaveAttribute("aria-valuenow", "5");
    expect(screen.getByRole("link", { name: "Open review" })).toHaveAttribute("href", "/analyses/a1");
    expect(screen.queryByRole("link", { name: "Resume review" })).toBeNull();
    expect(buttons()).toEqual([]);
  });

  it("finished: Resume review for a review the user started", () => {
    render(
      <AnalysisProgress
        state={{ state: "finished", total: 5 }}
        reviewStarted
        reviewHref="/analyses/a1"
        busy={false}
        onCancel={vi.fn()}
        onRetry={vi.fn()}
      />,
    );
    expect(screen.getByRole("link", { name: "Resume review" })).toHaveAttribute("href", "/analyses/a1");
  });

  it("disables the actions while a request runs", async () => {
    const { onRetry, onCancel } = show({ state: "failed", flow: null, message: "x" }, true);
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    await userEvent.click(screen.getByRole("button", { name: "Cancel analysis" }));
    expect(onRetry).not.toHaveBeenCalled();
    expect(onCancel).not.toHaveBeenCalled();
    for (const name of ["Retry", "Cancel analysis"]) {
      const button = screen.getByRole("button", { name });
      expect(button).toHaveAttribute("aria-disabled", "true");
      expect(button).not.toHaveAttribute("disabled");
      expect(button).toHaveAccessibleDescription(/Not available while the current request runs\./);
    }
  });

  it("every button and link has a tooltip; no banned wording in any state", () => {
    const states: AnalysisState[] = [
      { state: "planning" },
      { state: "running", done: 1, total: 3 },
      { state: "paused", resumes_at: null, done: 1, total: 3 },
      { state: "failed", flow: null, message: "Failed." },
      { state: "finished", total: 3 },
    ];
    for (const state of states) {
      const { unmount } = render(
        <AnalysisProgress
          state={state}
          reviewHref="/r"
          busy={false}
          onCancel={() => {}}
          onRetry={() => {}}
        />,
      );
      const region = screen.getByRole("region", { name: "Analysis progress" });
      for (const control of [
        ...within(region).queryAllByRole("button"),
        ...within(region).queryAllByRole("link"),
      ]) {
        expect(control).toHaveAccessibleDescription(/.+/);
      }
      expectTrustWording(region.textContent ?? "");
      unmount();
    }
  });

  describe("current stage and liveness (sse-events.md: stage, heartbeat)", () => {
    const flowStage = (started_at: string | null) => ({
      kind: "flow" as const,
      flow: { index: 3, title: "Default timeout 30 s → 5 s" },
      started_at,
    });
    it.each([
      [{ state: "planning", stage: { kind: "plan", started_at: startedAgo(MIN) } }, "Planning the review."],
      [
        { state: "running", done: 0, total: 4, stage: { kind: "pr_context", started_at: startedAgo(MIN) } },
        "Reading the PR context.",
      ],
      [
        { state: "running", done: 2, total: 4, stage: flowStage(startedAgo(MIN)) },
        "Analysing flow 3 of 4: Default timeout 30 s → 5 s",
      ],
      [
        { state: "running", done: 2, total: 4, stage: { kind: "flow", started_at: startedAgo(MIN) } },
        "Analysing a flow (2 of 4 still to do).",
      ],
      [
        { state: "running", done: 2, total: 4, stage: flowStage(null) },
        "Waiting for the runner; next: analysing flow 3 of 4: Default timeout 30 s → 5 s",
      ],
      [
        { state: "planning", stage: { kind: "plan", started_at: null } },
        "Waiting for the runner; next: planning the review.",
      ],
      // Parked: no flow job runs and none is admitted; nothing claims the server is working.
      [{ state: "running", done: 1, total: 4 }, "No flow job is running."],
      [{ state: "planning" }, "Between two jobs: the server starts the next one."],
    ] as [AnalysisState, string][])("names the stage from the server: %j", (state, text) => {
      show(state, false, activity());
      const line = screen.getByText(text);
      expect(line).toBeVisible();
      expect(line.closest('[aria-live="polite"]')).not.toBeNull();
      expectTrustWording(document.body.textContent ?? "");
    });

    it("shows an LLM flow title as plain text, on one line and shortened", () => {
      const title = `<b>Retry</b>\n${"x".repeat(200)}`;
      show(
        {
          state: "running",
          done: 0,
          total: 1,
          stage: { kind: "flow", flow: { index: 1, title }, started_at: null },
        },
        false,
        activity(),
      );
      const line = screen.getByText(/^Waiting for the runner; next: analysing flow 1 of 1: /);
      expect(line.querySelector("b")).toBeNull();
      expect(line.textContent).toContain("<b>Retry</b> xxx");
      expect(line.textContent!.endsWith("…")).toBe(true);
      expect(line.textContent!.length).toBeLessThan(160);
    });

    it.each([
      { state: "paused", resumes_at: null, done: 1, total: 3, stage: flowStage(null) },
      { state: "failed", flow: null, message: "Failed." },
      { state: "finished", total: 3 },
    ] as AnalysisState[])(
      "paused and ended analyses keep their own text, no stage line or indicator: %j",
      (state) => {
        show(state, false, activity());
        expect(
          screen.queryByText(
            /^(Planning the review|Reading the PR context|Analysing|Waiting for the runner|Between two jobs|No flow job)/,
          ),
        ).toBeNull();
        expect(indicator()).toBeNull();
      },
    );

    it("elapsed time from started_at on the server's clock (browser clock offset removed), never negative", () => {
      const state: AnalysisState = {
        state: "running",
        done: 2,
        total: 4,
        stage: flowStage(startedAgo(2 * MIN + 30_000)),
      };
      const { rerender } = show(state, false, activity());
      expect(indicator()).toHaveTextContent("Working · this stage started 2 min ago");
      const again = (a: ProgressActivity, st: AnalysisState = state) =>
        rerender(
          <AnalysisProgress
            state={st}
            reviewHref="/r"
            busy={false}
            onCancel={() => {}}
            onRetry={() => {}}
            activity={a}
          />,
        );
      // Browser clock 10 min ahead of the server: still 2 min.
      again(activity({ now: NOW + 10 * MIN, clockOffset: 10 * MIN, lastSignalAt: NOW + 10 * MIN }));
      expect(indicator()).toHaveTextContent("Working · this stage started 2 min ago");
      // Started "in the future" by a few seconds of clock jitter: not negative.
      again(activity(), { ...state, stage: flowStage(startedAgo(-5000)) });
      expect(indicator()).toHaveTextContent("Working · this stage started less than a minute ago");
      again(activity(), { ...state, stage: flowStage(startedAgo(75 * MIN)) });
      expect(indicator()).toHaveTextContent("Working · this stage started 1 h 15 min ago");
      // No heartbeat yet: the server's clock is not known, so no time is claimed.
      again(activity({ clockOffset: null }));
      expect(indicator()).toHaveTextContent(/^Working$/);
    });

    it("waiting for the runner: no indicator and no elapsed time", () => {
      show({ state: "running", done: 2, total: 4, stage: flowStage(null) }, false, activity());
      expect(indicator()).toBeNull();
      expect(screen.queryByText(/stage started/)).toBeNull();
    });

    it("a long flow with recent heartbeats stays Working", () => {
      show(
        { state: "running", done: 2, total: 4, stage: flowStage(startedAgo(40 * MIN)) },
        false,
        activity({ lastSignalAt: NOW - 15_000 }),
      );
      expect(indicator()).toHaveTextContent("Working · this stage started 40 min ago");
      expect(screen.queryByText(/No signal from the server/)).toBeNull();
    });

    it("keeps the elapsed time out of the live region", () => {
      show(
        { state: "planning", stage: { kind: "plan", started_at: startedAgo(2 * MIN) } },
        false,
        activity(),
      );
      expect(screen.getByText(/stage started/).closest("[aria-live]")).toBeNull();
    });

    it("no signal for more than 45 s: says so with the real time, stops the indicator and the bar's motion", () => {
      show(
        { state: "planning", stage: { kind: "plan", started_at: startedAgo(3 * MIN) } },
        false,
        activity({ lastSignalAt: NOW - SILENT_MS - 7000 }),
      );
      const notice = screen.getByText("No signal from the server for 52 s; reconnecting…");
      expect(notice.closest('[aria-live="polite"]')).not.toBeNull();
      expect(indicator()).toBeNull();
      expect(bar().querySelector("[class*='animate-']")).toBeNull();
      expect(screen.getByText("This stage started 3 min ago")).toBeVisible();
      expectTrustWording(document.body.textContent ?? "");
    });

    it("counts minutes after two minutes of silence; not silent just under 45 s", () => {
      const state: AnalysisState = {
        state: "planning",
        stage: { kind: "plan", started_at: startedAgo(MIN) },
      };
      const { unmount } = show(state, false, activity({ lastSignalAt: NOW - 3 * MIN - 1000 }));
      expect(screen.getByText("No signal from the server for 3 min; reconnecting…")).toBeVisible();
      unmount();
      show(state, false, activity({ lastSignalAt: NOW - SILENT_MS + 1000 }));
      expect(screen.queryByText(/No signal/)).toBeNull();
      expect(indicator()).not.toBeNull();
    });

    it("a dropped connection says it reconnects and shows no activity", () => {
      show(
        { state: "planning", stage: { kind: "plan", started_at: startedAgo(MIN) } },
        false,
        activity({ connected: false }),
      );
      expect(screen.getByText("Connection lost; reconnecting…")).toBeVisible();
      expect(indicator()).toBeNull();
    });

    it("under reduced motion nothing animates and the word Working is the static alternative", () => {
      show({ state: "planning", stage: { kind: "plan", started_at: startedAgo(MIN) } }, false, activity());
      for (const animated of document.querySelectorAll("[class*='animate-']")) {
        expect(animated.className).not.toMatch(/(^|\s)animate-/);
      }
      expect(indicator()).toHaveTextContent("Working");
      expect(indicator()!.querySelector(".motion-safe\\:animate-pulse")).not.toBeNull();
    });
  });
});

it("configuration failure asks the administrator and offers no Retry", () => {
  show({
    state: "failed",
    reason: "configuration",
    flow: null,
    message: "The server is misconfigured: a required agent skill is missing. Ask the administrator.",
  });
  expect(screen.getByText(/Ask the administrator/)).toBeVisible();
  expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
});

it("shows the plan part without resetting the whole-analysis clock", () => {
  const at = "2026-11-17T10:00:00Z";
  render(
    <AnalysisProgress
      state={{
        state: "planning",
        stage: { kind: "plan", part: { index: 6, total: 23 }, started_at: "2026-11-17T10:35:00Z" },
      }}
      progress={{
        started_at: at,
        planning: { done: 5, total: 23, state: "current" },
        pr_context: { done: 0, total: 1, state: "pending" },
        flows: { done: 0, total: null, state: "pending" },
        finishing: { done: 0, total: 1, state: "pending" },
        percent: 12,
        estimate: { remaining_seconds: 2400, basis: "this_analysis", rough: true },
      }}
      activity={{
        now: Date.parse("2026-11-17T10:40:00Z"),
        clockOffset: 0,
        lastSignalAt: Date.parse("2026-11-17T10:40:00Z"),
        connected: true,
      }}
      reviewHref="/review"
      busy={false}
      onCancel={() => {}}
      onRetry={() => {}}
    />,
  );
  expect(screen.getByText("Planning the review, part 6 of 23.")).toBeVisible();
  expect(screen.getByText(/Analysis started 40 min ago/)).toBeVisible();
  expect(screen.getByText(/this stage started 5 min ago/)).toBeVisible();
  expect(screen.getByText(/known after planning/)).toBeVisible();
  expect(screen.getByText(/About 40 min left.*rough estimate from this analysis/)).toBeVisible();
  expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "12");
});

it("shows provider-reported usage compactly", () => {
  render(
    <AnalysisProgress
      state={{ state: "planning" }}
      reviewHref="/review"
      busy={false}
      onCancel={() => {}}
      onRetry={() => {}}
      usage={{
        input_tokens: 1_200_000,
        cached_input_tokens: 900_000,
        output_tokens: 45_000,
        reasoning_output_tokens: null,
        jobs: 7,
      }}
    />,
  );
  expect(
    screen.getByText("LLM usage so far: 1.2 M input (0.9 M cached), 45 k output tokens in 7 jobs"),
  ).toBeVisible();
});

it("user pause resumes without a usage-limit claim and Cancel needs inline confirmation", async () => {
  const cancel = vi.fn();
  const resume = vi.fn();
  render(
    <AnalysisProgress
      state={{
        state: "paused",
        reason: "user",
        paused_by: "alice",
        paused_at: "2026-11-17T18:04:00Z",
        resumes_at: null,
        done: 3,
        total: 5,
      }}
      reviewHref="/review"
      busy={false}
      onCancel={cancel}
      onRetry={vi.fn()}
      onResume={resume}
    />,
  );
  expect(screen.getByText(/Paused by alice/)).toBeVisible();
  expect(screen.queryByText(/usage limit was reached/)).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Resume" }));
  expect(resume).toHaveBeenCalledOnce();
  await userEvent.click(screen.getByRole("button", { name: "Cancel analysis" }));
  expect(cancel).not.toHaveBeenCalled();
  expect(screen.getByRole("group", { name: "Confirm cancellation" })).toHaveTextContent(
    "Cancel discards the plan and the 3 analysed flows. The agent's session for this pull request is kept. Pause keeps everything.",
  );
  await userEvent.click(screen.getByRole("button", { name: "Confirm cancel" }));
  expect(cancel).toHaveBeenCalledOnce();
});

it("opens the review as soon as context is ready while flows are still running", () => {
  render(
    <AnalysisProgress
      state={{ state: "running", done: 1, total: 5 }}
      reviewReady
      busy={false}
      reviewHref="/analyses/a1"
      onCancel={() => {}}
      onRetry={() => {}}
    />,
  );
  expect(screen.getByText("1 of 5 flows analysed. The review is open.")).toBeVisible();
  expect(screen.queryByText("Between two jobs: the server starts the next one.")).not.toBeInTheDocument();
  expect(screen.getByRole("link", { name: "Open review" })).toHaveAttribute("href", "/analyses/a1");
  expect(screen.getByRole("button", { name: "Cancel analysis" })).toBeVisible();
});

it.each([
  [false, "Open"],
  [true, "Resume"],
])("parked while the review is open (started: %s): the line names the button, %s review", (started, verb) => {
  render(
    <AnalysisProgress
      state={{ state: "running", done: 1, total: 5 }}
      reviewReady
      reviewStarted={started}
      busy={false}
      reviewHref="/analyses/a1"
      onCancel={() => {}}
      onRetry={() => {}}
    />,
  );
  expect(screen.getByText(`No flow job is running. ${verb} the review to choose a flow.`)).toBeVisible();
  expect(screen.getByRole("link", { name: `${verb} review` })).toBeVisible();
});

it("plan ready with background off: not stuck; says so and offers Open review and Analyse all remaining flows", async () => {
  const onBackground = vi.fn();
  render(
    <AnalysisProgress
      state={{ state: "running", done: 1, total: 5 }}
      reviewReady
      backgroundAnalysis={false}
      onBackground={onBackground}
      busy={false}
      reviewHref="/analyses/a1"
      onCancel={() => {}}
      onRetry={() => {}}
    />,
  );
  expect(
    screen.getByText(
      "The plan is ready. Nothing is being analysed. Choose a flow to analyse, or analyse all remaining flows.",
    ),
  ).toBeVisible();
  expect(screen.getByRole("link", { name: "Open review" })).toBeVisible();
  // The remaining count is visible next to the button, not only in its tooltip.
  expect(screen.getByText("4 not analysed")).toBeVisible();
  await userEvent.click(screen.getByRole("button", { name: "Analyse all remaining flows" }));
  expect(onBackground).toHaveBeenCalledWith(true);
});

it("a failure without a flow after planning never claims it failed before planning", () => {
  const state: AnalysisState = {
    state: "failed",
    flow: null,
    message: "The repository mirror could not be updated.",
  };
  const props = { reviewHref: "/analyses/a1", busy: false, onCancel: () => {}, onRetry: () => {} };
  const { rerender } = render(
    <AnalysisProgress state={state} progress={failedAt("done", "done")} {...props} />,
  );
  expect(screen.getByText("The analysis failed after its flows were planned")).toBeVisible();
  expect(document.body).not.toHaveTextContent("before any flow was planned");
  rerender(<AnalysisProgress state={state} reviewReady {...props} />);
  expect(screen.getByText("The analysis failed after its flows were planned")).toBeVisible();
  // Without stage data the page does not guess when it failed.
  rerender(<AnalysisProgress state={state} {...props} />);
  expect(screen.getByText("The analysis failed")).toBeVisible();
});
