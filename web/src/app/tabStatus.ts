// The browser tab's title says whether diffpilot is still working or done, for a tab in the background
// (owner request, 2026-11-18: "diffpilot DET#1234: busy"). Screens feed their current activity; components never touch the title. No live
// region: the title is not announced.
import { useEffect } from "react";
import type {
  AnalysisState,
  AskQuestion,
  DashboardSettings,
  RefreshStatus,
  RepoRef,
} from "../api/generated/types";
import { useDashboardSettings } from "../api/queries";

/** Work shorter than this never touches the title. */
export const TAB_STATUS_THRESHOLD_MS = 10_000;
/** How long an outcome stays once the tab is visible and focused. */
export const TAB_STATUS_LINGER_MS = 4_000;
/** A screen giving up its activity keeps it this long, so the next screen can take it over with its start time. */
const HANDOVER_MS = 1_000;

/** The title's status word: `busy` while work runs, then `ready` or `attention` (failed or paused). */
export type TabStatusWord = "busy" | "ready" | "attention";
/** What a screen is doing: `busy`, the outcome of its work, or null when nothing. */
export type TabActivity = TabStatusWord | null;

interface Work {
  startedAt: number;
  timer: ReturnType<typeof setTimeout>;
  release?: ReturnType<typeof setTimeout>;
}

/**
 * The tab title over `document`: `<base> <subject>: <status>` (owner's format: "diffpilot DET#1234: busy"),
 * the subject being the page's PR or "My work", the status only for work that lasted the threshold.
 */
export function createTabStatus() {
  const base = document.title;
  const active = new Map<string, Work>();
  let subject: string | null = null;
  let outcome: TabStatusWord | null = null;
  let linger: ReturnType<typeof setTimeout> | undefined;

  const attentive = () => document.visibilityState === "visible" && document.hasFocus();
  const render = () => {
    const now = Date.now();
    const busy = [...active.values()].some((w) => now - w.startedAt >= TAB_STATUS_THRESHOLD_MS);
    const status = busy ? "busy" : outcome;
    const title = `${base}${subject ? ` ${subject}` : ""}${status ? `: ${status}` : ""}`;
    if (document.title !== title) document.title = title;
  };
  const fade = () => {
    clearTimeout(linger);
    linger = setTimeout(() => {
      outcome = null;
      render();
    }, TAB_STATUS_LINGER_MS);
  };
  const returned = () => {
    if (outcome !== null && attentive()) fade();
  };
  document.addEventListener("visibilitychange", returned);
  window.addEventListener("focus", returned);

  const drop = (key: string) => {
    const work = active.get(key);
    if (!work) return undefined;
    clearTimeout(work.timer);
    clearTimeout(work.release);
    active.delete(key);
    return work;
  };

  return {
    /** The page's subject (a PR's display id, "My work"); null for none. */
    setSubject(next: string | null): void {
      subject = next;
      render();
    },
    /** Starts the activity `key` (kept when already running); it shows once it has lasted the threshold. */
    start(key: string): void {
      const work = active.get(key);
      if (work) {
        clearTimeout(work.release);
        work.release = undefined;
      } else {
        active.set(key, { startedAt: Date.now(), timer: setTimeout(render, TAB_STATUS_THRESHOLD_MS) });
      }
      render();
    },
    /** Ends `key` with an outcome, shown only when the work lasted the threshold, until the user is back. */
    finish(key: string, word: "ready" | "attention"): void {
      const work = drop(key);
      if (!work) return;
      if (Date.now() - work.startedAt >= TAB_STATUS_THRESHOLD_MS) {
        outcome = word;
        if (attentive()) fade();
        else clearTimeout(linger);
      }
      render();
    },
    /** Ends `key` without an outcome, after a short handover to a screen that takes it over. */
    cancel(key: string): void {
      const work = active.get(key);
      if (!work || work.release) return;
      work.release = setTimeout(() => {
        drop(key);
        render();
      }, HANDOVER_MS);
    },
    /** Removes the listeners and restores the title (tests). */
    dispose(): void {
      for (const key of [...active.keys()]) drop(key);
      clearTimeout(linger);
      document.removeEventListener("visibilitychange", returned);
      window.removeEventListener("focus", returned);
      document.title = base;
    },
  };
}

let shared: ReturnType<typeof createTabStatus> | null = null;

const tabStatus = () => (shared ??= createTabStatus());

/** Feeds the tab title with a screen's activity `key` (screens only). */
export function useTabActivity(key: string, activity: TabActivity): void {
  useEffect(() => {
    const tab = tabStatus();
    if (activity === "busy") tab.start(key);
    else if (activity === null) tab.cancel(key);
    else tab.finish(key, activity);
  }, [key, activity]);
  useEffect(() => () => shared?.cancel(key), [key]);
}

/** Sets the title's subject while the screen shows it (screens only). */
export function useTabSubject(subject: string | null): void {
  useEffect(() => {
    tabStatus().setSubject(subject);
    return () => shared?.setSubject(null);
  }, [subject]);
}

/** A PR's display id: the repository's short name from the dashboard settings, else its name, and the number. */
export function prDisplayId(pr: { repo: RepoRef; number: number }, settings?: DashboardSettings): string {
  const rule = settings?.repositories.find(
    (r) => r.repo.owner === pr.repo.owner && r.repo.name === pr.repo.name,
  );
  return `${rule?.alias || pr.repo.name}#${pr.number}`;
}

/** The PR as the title's subject; the short names come from the dashboard settings when they can be read. */
export function usePrTabSubject(pr: { repo: RepoRef; number: number } | null | undefined): void {
  const settings = useDashboardSettings();
  useTabSubject(pr ? prDisplayId(pr, settings.data?.settings) : null);
}

/** The analysis's activity: busy while planning or analysing flows, then ready (idle or finished) or attention. */
export function analysisActivity(state: AnalysisState): TabActivity {
  switch (state.state) {
    case "planning":
      return "busy";
    case "running":
      return state.idle ? "ready" : "busy";
    case "finished":
      return "ready";
    case "failed":
    case "paused":
      return "attention";
    default:
      return null;
  }
}

/** The user's own questions: answering while one is open, else the newest one's outcome. */
export function questionsActivity(questions: readonly AskQuestion[]): TabActivity {
  const mine = questions.filter((q) => q.mine !== false);
  if (mine.some((q) => q.state === "pending" || q.state === "answering")) return "busy";
  const newest = [...mine].sort((a, b) => b.created_at.localeCompare(a.created_at))[0];
  if (newest?.state === "answered") return "ready";
  if (newest?.state === "failed") return "attention";
  return null;
}

/** The dashboard's latest refresh: busy while scanning or ranking, then its outcome. */
export function dashboardActivity(refresh: RefreshStatus | null | undefined): TabActivity {
  switch (refresh?.state) {
    case "scanning":
    case "ranking":
      return "busy";
    case "finished":
      return "ready";
    case "failed":
      return "attention";
    default:
      return null;
  }
}

/** The dashboard in the tab title: "My work" and its refresh (one call in the dashboard screen). */
export function useDashboardTabStatus(refresh: RefreshStatus | null | undefined): void {
  useTabSubject("My work");
  useTabActivity("dashboard", dashboardActivity(refresh));
}
