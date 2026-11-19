import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AnalysisState, AnalysisSummary, AskQuestion, Flow, Tour } from "../api/generated/types";
import { expectTrustWording } from "../__tests__/wording";
import {
  NOTIFY_HANDOVER_MS,
  disableNotifications,
  enableNotifications,
  notificationsOn,
  notifySupport,
  notifyWorkDone,
  progressEvents,
  questionEvents,
  tourEvents,
  type WorkEvent,
} from "./workNotifications";

class FakeNotification {
  static permission: NotificationPermission = "granted";
  static requestPermission = vi.fn(async () => FakeNotification.permission);
  static shown: FakeNotification[] = [];
  onclick: ((event: Event) => void) | null = null;
  close = vi.fn();
  constructor(
    public title: string,
    public options: NotificationOptions,
  ) {
    FakeNotification.shown.push(this);
  }
}

let visible = true;
let focused = true;
beforeEach(() => {
  vi.useFakeTimers();
  localStorage.clear();
  visible = true;
  focused = true;
  FakeNotification.permission = "granted";
  FakeNotification.shown = [];
  FakeNotification.requestPermission.mockClear();
  vi.stubGlobal("Notification", FakeNotification);
  vi.stubGlobal("isSecureContext", true);
  vi.spyOn(document, "visibilityState", "get").mockImplementation(() => (visible ? "visible" : "hidden"));
  vi.spyOn(document, "hasFocus").mockImplementation(() => focused);
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const EVENT: WorkEvent = {
  key: "a1:question:q1",
  title: "Answer ready — NC#482",
  body: "LLM answer to your question on flow 1, step 5.",
  path: "/analyses/a1#s5",
};

/** Lets the handover delay and the claim run. */
const settle = async (pending: Promise<void>) => {
  await vi.advanceTimersByTimeAsync(NOTIFY_HANDOVER_MS);
  await pending;
};

describe("support and opt-in", () => {
  it("says why notifications cannot work: unsupported, insecure (http), blocked", () => {
    vi.stubGlobal("isSecureContext", false);
    expect(notifySupport()).toBe("insecure");
    vi.stubGlobal("isSecureContext", true);
    FakeNotification.permission = "denied";
    expect(notifySupport()).toBe("denied");
    FakeNotification.permission = "default";
    expect(notifySupport()).toBe("default");
    vi.stubGlobal("Notification", undefined);
    expect(notifySupport()).toBe("unsupported");
  });

  it("is off by default; turning it on asks for permission and is stored when granted", async () => {
    FakeNotification.permission = "default";
    expect(notificationsOn()).toBe(false);
    FakeNotification.requestPermission.mockImplementationOnce(async () => {
      FakeNotification.permission = "granted";
      return "granted";
    });
    await expect(enableNotifications()).resolves.toBe("granted");
    expect(FakeNotification.requestPermission).toHaveBeenCalledTimes(1);
    expect(notificationsOn()).toBe(true);
    disableNotifications();
    expect(notificationsOn()).toBe(false);
  });

  it("stays off when the browser refuses; a later block turns a stored choice off", async () => {
    FakeNotification.permission = "default";
    FakeNotification.requestPermission.mockImplementationOnce(async () => "denied");
    await expect(enableNotifications()).resolves.toBe("denied");
    expect(notificationsOn()).toBe(false);
    FakeNotification.permission = "granted";
    await enableNotifications();
    expect(notificationsOn()).toBe(true);
    FakeNotification.permission = "denied";
    expect(notificationsOn()).toBe(false);
  });
});

describe("notifyWorkDone", () => {
  beforeEach(() => {
    localStorage.setItem("diffpilot.notify-when-done", "on");
  });

  it("shows nothing while the choice is off", async () => {
    disableNotifications();
    visible = false;
    await settle(notifyWorkDone(EVENT, vi.fn()));
    expect(FakeNotification.shown).toHaveLength(0);
  });

  it("shows nothing in a visible, focused tab: the page itself says it", async () => {
    await settle(notifyWorkDone(EVENT, vi.fn()));
    expect(FakeNotification.shown).toHaveLength(0);
  });

  it("notifies a hidden tab once, with a tag per event and no answer text", async () => {
    visible = false;
    focused = false;
    await settle(notifyWorkDone(EVENT, vi.fn()));
    expect(FakeNotification.shown).toHaveLength(1);
    const [shown] = FakeNotification.shown;
    expect(shown!.title).toBe(EVENT.title);
    expect(shown!.options).toMatchObject({ body: EVENT.body, tag: "diffpilot:a1:question:q1" });
  });

  it("a user back in the tab before the handover ends gets no notification", async () => {
    visible = false;
    const pending = notifyWorkDone(EVENT, vi.fn());
    visible = true;
    await settle(pending);
    expect(FakeNotification.shown).toHaveLength(0);
  });

  it("notifies a visible tab of an unfocused window", async () => {
    focused = false;
    await settle(notifyWorkDone(EVENT, vi.fn()));
    expect(FakeNotification.shown).toHaveLength(1);
  });

  it("the same event again after the cross-tab window (a re-run within minutes) notifies again", async () => {
    visible = false;
    await settle(notifyWorkDone(EVENT, vi.fn()));
    await vi.advanceTimersByTimeAsync(3 * 60_000);
    await settle(notifyWorkDone(EVENT, vi.fn()));
    expect(FakeNotification.shown).toHaveLength(2);
  });

  it("one notification per event across tabs (shared browser storage)", async () => {
    visible = false;
    await settle(
      Promise.all([notifyWorkDone(EVENT, vi.fn()), notifyWorkDone(EVENT, vi.fn())]).then(() => {}),
    );
    await settle(notifyWorkDone(EVENT, vi.fn()));
    expect(FakeNotification.shown).toHaveLength(1);
  });

  it("an event a watching tab saw is not notified by a background tab", async () => {
    await settle(notifyWorkDone(EVENT, vi.fn()));
    visible = false;
    await settle(notifyWorkDone(EVENT, vi.fn()));
    expect(FakeNotification.shown).toHaveLength(0);
  });

  it("a click focuses the tab, opens the event's page and closes the notification", async () => {
    visible = false;
    const focus = vi.spyOn(window, "focus").mockImplementation(() => {});
    const open = vi.fn();
    await settle(notifyWorkDone(EVENT, open));
    const [shown] = FakeNotification.shown;
    shown!.onclick!(new Event("click"));
    expect(focus).toHaveBeenCalled();
    expect(open).toHaveBeenCalledWith("/analyses/a1#s5");
    expect(shown!.close).toHaveBeenCalled();
  });

  it("a browser whose constructor throws (mobile) shows nothing and does not fail", async () => {
    visible = false;
    vi.stubGlobal(
      "Notification",
      Object.assign(
        function () {
          throw new TypeError("Illegal constructor");
        },
        { permission: "granted" },
      ),
    );
    await expect(settle(notifyWorkDone(EVENT, vi.fn()))).resolves.toBeUndefined();
  });
});

const PR = "NC#482";
const flow = (
  id: string,
  position: number,
  state?: string,
  steps: { id: string; number: number }[] = [],
  started_at: string | null = null,
) =>
  ({
    id,
    position,
    analysis_status: state && { state, started_at, message: null },
    steps,
  }) as unknown as Flow;
const tour = (state: AnalysisState, flows: Flow[], finished_at: string | null = null) =>
  ({ analysis: { id: "a1", state, finished_at }, flows }) as unknown as Tour;
const running: AnalysisState = { state: "running", done: 1, total: 3, idle: false };

describe("events of the review", () => {
  it("a flow analysed or failed, with its position and a link to its introduction", () => {
    const before = tour(running, [
      flow("f1", 1, "analysed"),
      flow("f2", 2, "analysing"),
      flow("f3", 3, "queued"),
    ]);
    const after = tour(running, [
      flow("f1", 1, "analysed"),
      flow("f2", 2, "analysed"),
      flow("f3", 3, "failed"),
    ]);
    const events = tourEvents(before, after, PR);
    expect(events.map((e) => [e.title, e.path])).toEqual([
      ["Flow 2 analysed — NC#482", "/analyses/a1#f2"],
      ["Flow 3 failed — NC#482", "/analyses/a1#f3"],
    ]);
    expect(events[0]!.body).toMatch(/LLM/);
    expect(new Set(events.map((e) => e.key)).size).toBe(2);
  });

  it("an analysis that finishes or fails in the same refetch as its last flow says both", () => {
    const before = tour(running, [flow("f1", 1, "analysed"), flow("f2", 2, "analysing")]);
    const finished = tour(
      { state: "finished", total: 2 },
      [flow("f1", 1, "analysed"), flow("f2", 2, "analysed")],
      "2026-11-19T10:00:00Z",
    );
    expect(tourEvents(before, finished, PR).map((e) => e.title)).toEqual([
      "Flow 2 analysed — NC#482",
      "Analysis finished — NC#482",
    ]);
    const failed = tour(
      { state: "failed", flow: null, message: "x" },
      [flow("f1", 1, "analysed"), flow("f2", 2, "failed")],
      "2026-11-19T10:00:00Z",
    );
    const events = tourEvents(before, failed, PR);
    expect(events.map((e) => [e.title, e.path])).toEqual([
      ["Flow 2 failed — NC#482", "/analyses/a1#f2"],
      ["Analysis failed — NC#482", "/analyses/a1"],
    ]);
    expect(events[1]!.body).not.toContain("x");
  });

  it("an analysis reopened by a flow request and finished again says so again, under a new key", () => {
    const flows = [flow("f1", 1, "analysed")];
    const first = tour({ state: "finished", total: 1 }, flows, "2026-11-19T10:00:00Z");
    const reopened = tour(running, [flow("f1", 1, "analysing", [], "2026-11-19T10:05:00Z")]);
    const again = tour({ state: "finished", total: 1 }, flows, "2026-11-19T10:09:00Z");
    const finishedFirst = tourEvents(tour(running, [flow("f1", 1, "analysing")]), first, PR);
    const finishedAgain = tourEvents(reopened, again, PR);
    expect(finishedAgain.map((e) => e.title)).toEqual([
      "Flow 1 analysed — NC#482",
      "Analysis finished — NC#482",
    ]);
    expect(finishedAgain.at(-1)!.key).not.toBe(finishedFirst.at(-1)!.key);
    // A refetch that missed the reopening still sees a new finish.
    expect(tourEvents(first, again, PR).map((e) => e.title)).toEqual(["Analysis finished — NC#482"]);
  });

  it("one key for a flow's completion whether a tab saw it queued or analysing (one notification across tabs)", () => {
    const after = tour(running, [flow("f1", 1, "analysed")]);
    const fromAnalysing = tourEvents(
      tour(running, [flow("f1", 1, "analysing", [], "2026-11-19T10:00:00Z")]),
      after,
      PR,
    );
    const fromQueued = tourEvents(tour(running, [flow("f1", 1, "queued")]), after, PR);
    expect(fromQueued[0]!.key).toBe(fromAnalysing[0]!.key);
  });

  it("texts never promise what the server may not do: review ready is neutral, a failed flow asks nothing", () => {
    const failed = tourEvents(
      tour(running, [flow("f1", 1, "analysing")]),
      tour(running, [flow("f1", 1, "failed")]),
      PR,
    );
    expect(failed[0]!.body).toBe("The LLM analysis of flow 1 failed. Open it for details.");
  });

  it("nothing for an unchanged tour, flows without work status or a flow analysed again from the start", () => {
    const a = tour(running, [flow("f1", 1), flow("f2", 2, "analysed")]);
    expect(tourEvents(a, a, PR)).toEqual([]);
    expect(tourEvents(a, tour(running, [flow("f1", 1), flow("f2", 2, "queued")]), PR)).toEqual([]);
  });

  it("an answer or a failure of the user's own question, with flow and step, never the answer text", () => {
    const t = tour(running, [flow("f1", 1, "analysed", [{ id: "s5", number: 5 }])]);
    const q = (id: string, state: AskQuestion["state"], mine = true) =>
      ({ id, step_id: "s5", state, mine, text: "Why retry?", answer: null }) as unknown as AskQuestion;
    const before = [
      q("q1", "answering"),
      q("q2", "pending"),
      q("q3", "answering", false),
      q("q4", "answered"),
    ];
    const after = [q("q1", "answered"), q("q2", "failed"), q("q3", "answered", false), q("q4", "answered")];
    const events = questionEvents("a1", before, after, t, PR);
    expect(events.map((e) => [e.title, e.body, e.path])).toEqual([
      ["Answer ready — NC#482", "LLM answer to your question on flow 1, step 5.", "/analyses/a1#s5"],
      ["Question failed — NC#482", "No LLM answer to your question on flow 1, step 5.", "/analyses/a1#s5"],
    ]);
    expect(events.map((e) => e.body).join()).not.toContain("Why retry");
  });

  it("a question first seen already answered (asked before this page) is not new work", () => {
    const t = tour(running, []);
    const q = { id: "q9", step_id: "s1", state: "answered", mine: true } as unknown as AskQuestion;
    expect(questionEvents("a1", [], [q], t, PR)).toEqual([]);
  });
});

describe("events of the progress page", () => {
  const summary = (state: AnalysisState, review_ready = false, id = "a1") =>
    ({ id, state, review_ready }) as unknown as AnalysisSummary;

  it("review ready, then finished; a failure links to the progress page", () => {
    const planning = summary({ state: "planning" });
    const ready = summary(running, true);
    expect(progressEvents(planning, ready, PR).map((e) => [e.title, e.path])).toEqual([
      ["Review ready — NC#482", "/analyses/a1"],
    ]);
    expect(progressEvents(planning, ready, PR)[0]!.body).toBe("Plan and PR context are ready.");
    expect(
      progressEvents(ready, summary({ state: "finished", total: 3 }, true), PR).map((e) => e.title),
    ).toEqual(["Analysis finished — NC#482"]);
    const failed = summary({ state: "failed", flow: null, message: "x" });
    expect(progressEvents(planning, failed, PR).map((e) => [e.title, e.path])).toEqual([
      ["Analysis failed — NC#482", "/analyses/a1/progress"],
    ]);
  });

  it("nothing when the page switches to another analysis or nothing changed", () => {
    expect(progressEvents(summary({ state: "planning" }), summary(running, true, "a2"), PR)).toEqual([]);
    expect(progressEvents(summary(running, true), summary(running, true), PR)).toEqual([]);
  });

  it("every text keeps the trust wording", () => {
    const texts = [
      ...progressEvents(summary({ state: "planning" }), summary(running, true), PR),
      ...progressEvents(summary(running, true), summary({ state: "finished", total: 3 }, true), PR),
      ...progressEvents(summary(running), summary({ state: "failed", flow: null, message: "x" }), PR),
    ].flatMap((e) => [e.title, e.body]);
    expectTrustWording(texts.join("\n"));
  });
});
