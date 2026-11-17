// Screen scenarios of the msw mocks (Home, progress, Settings), selected with `?scenario=<name>`; orthogonal to
// the fixture states of fixtures.ts. Settings have no api-seed yet, so their default answer lives here.
import type {
  AnalysisState,
  AnalysisSummary,
  MeResponse,
  RepoSettings,
  ResolvePrResponse,
  Settings,
} from "../api/generated/types";
import { seedNow } from "./fixtures";

/** Head of the fixture PR after it "moved" (scenario `head-moved`). */
export const MOVED_HEAD = "d41b7a0c8e5f3a2b1c0d9e8f7a6b5c4d3e2f1a0b";

/** Default settings (http-api.md inventory: working, en, 1 repository). */
export const SETTINGS: Settings = {
  llm_status: { at: "2026-11-16T09:31:00Z", status: { state: "working" } },
  language: "en",
  app_installations: [{ account: "acme", all_repositories: false }],
  repos: [{ owner: "acme", name: "node-client" }],
};

/** Default read-only settings of acme/node-client. */
export const REPO_SETTINGS: RepoSettings = {
  repo: { owner: "acme", name: "node-client" },
  rules: [
    { kind: "path_glob", pattern: "tests/**", category: "tests" },
    { kind: "path_glob", pattern: "docs/**", category: "docs_comments" },
    { kind: "path_glob", pattern: "Cargo.lock", category: "generated_mechanical" },
    { kind: "line_regex", pattern: "^\\s*//", category: "docs_comments" },
  ],
  related_repos: [{ owner: "acme", name: "node-sdk" }],
  project_profile: { sources: ["repository_file"] },
};

interface Scenario {
  /** `me` answer. */
  me?: () => MeResponse;
  /** Changes to the `pr/resolve` answer of the fixture PR. */
  resolve?: (seed: ResolvePrResponse) => ResolvePrResponse;
  /** The analysis stays in this state (no scripted run after Analyze). */
  analysis?: AnalysisState;
  /** `settings` answer. */
  settings?: Settings;
  /** `repos/{owner}/{name}/settings` answer. */
  repoSettings?: RepoSettings;
}

const analysisAt = (state: AnalysisState): AnalysisSummary => ({
  ...(seedNow("analysis") as AnalysisSummary),
  state,
});

const withAnalysis = (state: AnalysisState) => (seed: ResolvePrResponse) => ({
  ...seed,
  analysis: analysisAt(state),
});

const SCENARIOS = {
  "signed-out": { me: () => ({ user: null, csrf_token: null }) },
  "head-moved": {
    resolve: (seed: ResolvePrResponse) => ({
      ...seed,
      pr: { ...seed.pr, head_sha: MOVED_HEAD },
      analysis: analysisAt({ state: "finished", total: 5 }),
      head_moved: true,
    }),
  },
  "existing-running": {
    resolve: withAnalysis({ state: "running", done: 2, total: 5 }),
    analysis: { state: "running", done: 2, total: 5 },
  },
  "existing-failed": {
    resolve: withAnalysis({ state: "failed", flow: null, message: "Planning the flows failed." }),
    analysis: { state: "failed", flow: null, message: "Planning the flows failed." },
  },
  "existing-cancelled": { resolve: withAnalysis({ state: "cancelled" }) },
  "no-app": {
    resolve: (seed: ResolvePrResponse) => ({ ...seed, commenting_available: false }),
    settings: { ...SETTINGS, app_installations: [] },
  },
  planning: { analysis: { state: "planning" } },
  running: { analysis: { state: "running", done: 2, total: 5 } },
  paused: { analysis: { state: "paused", resumes_at: "2026-11-17T14:20:00Z" } },
  "paused-unknown": { analysis: { state: "paused", resumes_at: null } },
  failed: {
    analysis: {
      state: "failed",
      flow: { id: "f3", title: "Classify RPC errors" },
      message: "The LLM answer did not match the expected format after 2 repairs.",
    },
  },
  "failed-early": {
    analysis: {
      state: "failed",
      flow: null,
      message: "The pull request could not be fetched from GitHub. GitHub answered with an error.",
    },
  },
  finished: { analysis: { state: "finished", total: 5 } },
  "llm-error": {
    settings: {
      ...SETTINGS,
      llm_status: {
        at: "2026-11-17T08:05:00Z",
        status: { state: "not_working", message: "Codex usage limit reached. It resets at 14:20." },
      },
    },
  },
  "llm-unknown": {
    settings: { ...SETTINGS, llm_status: null, language: "pl" },
    repoSettings: { ...REPO_SETTINGS, related_repos: [], project_profile: { sources: [] } },
  },
} satisfies Record<string, Scenario>;

/** Name of a screen scenario. */
export type ScenarioName = keyof typeof SCENARIOS;

/** All scenario names. */
export const SCENARIO_NAMES = Object.keys(SCENARIOS) as ScenarioName[];

let current: ScenarioName | null = null;

/** Selects a scenario (null = none: fixture answers and a scripted run after Analyze). */
export function setScenario(name: ScenarioName | null): void {
  if (name !== null && !(name in SCENARIOS)) throw new Error(`unknown scenario "${String(name)}"`);
  current = name;
}

/** The selected scenario's overrides. */
export function scenario(): Scenario {
  return current === null ? {} : SCENARIOS[current];
}
