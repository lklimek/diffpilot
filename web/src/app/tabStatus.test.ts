import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AnalysisState, AskQuestion, DashboardSettings, RefreshStatus } from "../api/generated/types";
import {
  TAB_STATUS_LINGER_MS,
  TAB_STATUS_THRESHOLD_MS,
  analysisActivity,
  createTabStatus,
  dashboardActivity,
  prDisplayId,
  questionsActivity,
} from "./tabStatus";

let visible = true;
let focused = true;
beforeEach(() => {
  vi.useFakeTimers();
  visible = true;
  focused = true;
  document.title = "diffpilot";
  vi.spyOn(document, "visibilityState", "get").mockImplementation(() => (visible ? "visible" : "hidden"));
  vi.spyOn(document, "hasFocus").mockImplementation(() => focused);
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const away = () => {
  visible = false;
  focused = false;
  document.dispatchEvent(new Event("visibilitychange"));
};
const back = () => {
  visible = true;
  focused = true;
  document.dispatchEvent(new Event("visibilitychange"));
  window.dispatchEvent(new Event("focus"));
};

describe("tab status (owner's format: diffpilot DET#1234: busy)", () => {
  it("shows busy only once work has lasted 10 s; shorter work never touches the title", () => {
    const tab = createTabStatus();
    tab.setSubject("dash-evo-tool#1234");
    expect(document.title).toBe("diffpilot dash-evo-tool#1234");
    tab.start("pr");
    vi.advanceTimersByTime(TAB_STATUS_THRESHOLD_MS - 1);
    expect(document.title).toBe("diffpilot dash-evo-tool#1234");
    tab.finish("pr", "ready");
    vi.advanceTimersByTime(TAB_STATUS_THRESHOLD_MS);
    expect(document.title).toBe("diffpilot dash-evo-tool#1234");
    tab.start("analysis");
    vi.advanceTimersByTime(TAB_STATUS_THRESHOLD_MS);
    expect(document.title).toBe("diffpilot dash-evo-tool#1234: busy");
    tab.dispose();
  });

  it("without a subject (review start before a PR is known): diffpilot: busy", () => {
    const tab = createTabStatus();
    tab.start("pr");
    vi.advanceTimersByTime(TAB_STATUS_THRESHOLD_MS);
    expect(document.title).toBe("diffpilot: busy");
    tab.dispose();
  });

  it("keeps ready while the tab is in the background, until it is visible and focused again", () => {
    const tab = createTabStatus();
    tab.setSubject("DET#7");
    tab.start("analysis");
    vi.advanceTimersByTime(TAB_STATUS_THRESHOLD_MS);
    away();
    tab.finish("analysis", "ready");
    expect(document.title).toBe("diffpilot DET#7: ready");
    vi.advanceTimersByTime(60_000);
    expect(document.title).toBe("diffpilot DET#7: ready");
    back();
    vi.advanceTimersByTime(TAB_STATUS_LINGER_MS);
    expect(document.title).toBe("diffpilot DET#7");
    tab.dispose();
  });

  it("in a tab watched the whole time the outcome lingers only a few seconds; attention reads so", () => {
    const tab = createTabStatus();
    tab.start("ask");
    vi.advanceTimersByTime(TAB_STATUS_THRESHOLD_MS);
    tab.finish("ask", "attention");
    expect(document.title).toBe("diffpilot: attention");
    vi.advanceTimersByTime(TAB_STATUS_LINGER_MS);
    expect(document.title).toBe("diffpilot");
    tab.dispose();
  });

  it("several activities: busy while any shown one runs; the outcome after the last", () => {
    const tab = createTabStatus();
    tab.start("analysis");
    vi.advanceTimersByTime(5_000);
    tab.start("ask");
    vi.advanceTimersByTime(5_000);
    expect(document.title).toBe("diffpilot: busy");
    away();
    vi.advanceTimersByTime(5_000);
    tab.finish("ask", "ready");
    expect(document.title).toBe("diffpilot: busy");
    tab.finish("analysis", "attention");
    expect(document.title).toBe("diffpilot: attention");
    tab.dispose();
  });

  it("an outcome without a start (a page opened on finished work) changes nothing", () => {
    const tab = createTabStatus();
    tab.finish("analysis", "ready");
    expect(document.title).toBe("diffpilot");
    tab.dispose();
  });

  it("a PR's display id: the short name from the dashboard settings, else the repository name", () => {
    const pr = { repo: { owner: "dashpay", name: "dash-evo-tool" }, number: 1234 };
    expect(prDisplayId(pr)).toBe("dash-evo-tool#1234");
    const settings = { repositories: [{ repo: pr.repo, alias: "DET" }] } as unknown as DashboardSettings;
    expect(prDisplayId(pr, settings)).toBe("DET#1234");
  });
});

describe("activities", () => {
  it("analysis: busy while planning or running, ready when idle or finished, attention when failed or paused", () => {
    const cases: [AnalysisState, unknown][] = [
      [{ state: "planning" }, "busy"],
      [{ state: "running", done: 1, total: 7, idle: false }, "busy"],
      [{ state: "running", done: 1, total: 7, idle: true }, "ready"],
      [{ state: "finished", total: 7 }, "ready"],
      [{ state: "failed", flow: null, message: "x" }, "attention"],
      [{ state: "paused", resumes_at: null, done: 1, total: 7 }, "attention"],
      [{ state: "cancelled" } as AnalysisState, null],
    ];
    for (const [state, activity] of cases) expect(analysisActivity(state)).toEqual(activity);
  });

  it("ask: only the user's own questions; busy while answering, then the newest one's outcome", () => {
    const q = (state: AskQuestion["state"], created_at: string, mine = true) =>
      ({ state, created_at, mine }) as AskQuestion;
    expect(questionsActivity([q("answered", "1"), q("answering", "2")])).toBe("busy");
    expect(questionsActivity([q("answered", "2"), q("failed", "1")])).toBe("ready");
    expect(questionsActivity([q("failed", "2"), q("answered", "1")])).toBe("attention");
    expect(questionsActivity([q("pending", "3", false)])).toBeNull();
    expect(questionsActivity([])).toBeNull();
  });

  it("dashboard refresh: busy while scanning or ranking, then its outcome", () => {
    const r = (state: RefreshStatus["state"]) => ({ state }) as RefreshStatus;
    expect(dashboardActivity(r("scanning"))).toBe("busy");
    expect(dashboardActivity(r("ranking"))).toBe("busy");
    expect(dashboardActivity(r("finished"))).toBe("ready");
    expect(dashboardActivity(r("failed"))).toBe("attention");
    expect(dashboardActivity(r("cancelled"))).toBeNull();
    expect(dashboardActivity(null)).toBeNull();
  });
});
