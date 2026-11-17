// Screen scenarios of the msw mocks (Home, progress, Settings), selected with `?scenario=<name>`; orthogonal to
// the fixture states of fixtures.ts. Settings have no api-seed yet, so their default answer lives here.
import type {
  AnalysisState,
  AnalysisSummary,
  AskQuestion,
  MeResponse,
  RepoSettings,
  ResolvePrResponse,
  Settings,
  Tour,
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
  /** Changes to the `tour` answer. */
  tour?: (seed: Tour) => Tour;
  /** Changes to the stored questions. */
  questions?: (seed: AskQuestion[]) => AskQuestion[];
}

/** Markup and a script URL in agent output (threat model T12): the page must show them as text. */
export const XSS = {
  markup: '<img src=x onerror="window.__xss=1"><script>window.__xss=2</script>',
  url: "javascript:window.__xss=3",
} as const;

/** The seed tour with markup in LLM strings and script URLs in LLM-adjacent links. */
function withXss(seed: Tour): Tour {
  const tour = structuredClone(seed);
  const flow = tour.flows[0]!;
  const step = flow.steps[0]!;
  flow.llm.title = `${XSS.markup} retry`;
  flow.llm.introduction.problem_before = XSS.markup;
  step.llm.title = `${XSS.markup} trigger`;
  step.llm.why = XSS.markup;
  step.facts.github_url = XSS.url;
  for (const annotation of flow.steps.flatMap((s) => s.llm.annotations)) {
    annotation.was = XSS.markup;
    annotation.text = XSS.markup;
  }
  tour.pr_context.llm.problem = XSS.markup;
  tour.pr_context.llm.related_work = tour.pr_context.llm.related_work.map((w) => ({
    ...w,
    reference: XSS.url,
  }));
  tour.threads = tour.threads.map((t) => ({ ...t, excerpt: XSS.markup, url: XSS.url }));
  for (const s of flow.steps) {
    s.facts.github_url = XSS.url;
    s.llm.impact = s.llm.impact.map((i) => ({ ...i, reason: XSS.markup, github_url: XSS.url }));
  }
  flow.llm.pattern_repetitions = flow.llm.pattern_repetitions.map((p) => ({
    ...p,
    description: XSS.markup,
    sites: p.sites.map((site) => ({ ...site, github_url: XSS.url })),
  }));
  flow.graph.nodes = flow.graph.nodes.map((n) => ({ ...n, label: `${XSS.markup} ${n.number}` }));
  flow.graph.edges = flow.graph.edges.map((e) => ({ ...e, label: e.label === null ? null : XSS.markup }));
  return tour;
}

/** A pinned LLM answer with markup on the first step of flow 1. */
const xssQuestions = (seed: AskQuestion[]): AskQuestion[] => [
  ...seed,
  {
    id: "3f1c5a7e-0482-4d2b-9e6f-0000000000x5",
    step_id: "f1.1",
    text: XSS.markup,
    state: "answered",
    answer: { answer: `${XSS.markup} answer`, evidence: [] },
    failure: null,
    created_at: "2026-11-16T10:30:00Z",
  },
];

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
  xss: { tour: withXss, questions: xssQuestions },
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
