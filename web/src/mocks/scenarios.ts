// Screen scenarios of the msw mocks (Home, progress, Settings), selected with `?scenario=<name>`; orthogonal to
// the fixture states of fixtures.ts. Default settings come from the api-seed; repository settings have none yet.
import type {
  UsageTotals,
  AnalysisProgress,
  EstimateBasis,
  AnalysisStage,
  AnalysisState,
  AnalysisSummary,
  FeedbackMemory,
  PendingReview,
  AskQuestion,
  LinesResponse,
  MeResponse,
  RepoSettings,
  ResolvePrResponse,
  Settings,
  StageKind,
  ThreadsStatus,
  Tour,
} from "../api/generated/types";
import { seedFor, seedNow } from "./fixtures";
import { REAL_QUESTIONS, REAL_TOURS } from "./real";

/** When this module loaded (the page in the browser): the start of the scenarios' running stages. */
const LOADED = new Date().toISOString();
const now = () => new Date().toISOString();
/** Delay of the first scripted step (as `RUN_STEP_MS` of live-analysis, which imports this module). */
const RUN_STEP_MS = 400;

/** Titles of the fixture flows, in plan order. */
export const FLOW_TITLES = (seedFor("default", "tour") as Tour).flows.map((f) => f.llm.title);

/** A stage of kind `kind` claimed at `started_at` (null: waiting for the runner). */
const stage = (kind: StageKind, started_at: string | null): AnalysisStage => ({ kind, started_at });

/** The stage of flow `index` (1-based) of the fixture plan. */
export const flowStage = (index: number, started_at: string | null): AnalysisStage => ({
  kind: "flow",
  flow: { index, title: FLOW_TITLES[index - 1] ?? `Flow ${index}` },
  started_at,
});

/** Head of the fixture PR after it "moved" (scenario `head-moved`). */
export const MOVED_HEAD = "d41b7a0c8e5f3a2b1c0d9e8f7a6b5c4d3e2f1a0b";

/** Default settings: the api-seed (working, en, 1 installation, 1 repository). */
export const SETTINGS = seedFor("default", "settings") as Settings;

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
  project_profile: { sources: ["repository_file"], memcan: "not_checked" },
};

interface Scenario {
  showLess?: boolean;
  lazy?: boolean;
  /** `background_analysis` of a lazy analysis (default off: flows run on request only). */
  background?: boolean;
  llmSaveFails?: boolean;
  manyRepos?: boolean;
  /** `me` answer. */
  me?: () => MeResponse;
  /** Changes to the `pr/resolve` answer of the fixture PR. */
  resolve?: (seed: ResolvePrResponse) => ResolvePrResponse;
  /** The analysis stays in this state (no scripted run after Start analysis). */
  analysis?: AnalysisState;
  usage?: UsageTotals;
  /** Waiting facts, projected on each scripted stage change. */
  progress?: (state: AnalysisState) => AnalysisProgress;
  /** States made and published, each after its delay (ms), once the first event stream opens; the last one stays. */
  script?: [number, () => AnalysisState][];
  /** No heartbeats, and every event stream after the first stays silent (a dead server behind an open socket). */
  silent?: true;
  /** `settings` answer. */
  settings?: Settings;
  /** `repos/{owner}/{name}/settings` answer; null = not known yet (409 `analysis_not_finished`). */
  repoSettings?: RepoSettings | null;
  /** Changes to the `tour` answer. */
  tour?: (seed: Tour) => Tour;
  /** Changes to a `lines` answer. */
  lines?: (answer: LinesResponse) => LinesResponse;
  /** Changes to the stored questions. */
  questions?: (seed: AskQuestion[]) => AskQuestion[];
  /** Outcome of the MemCan write of feedback (default `written`). */
  memcan?: FeedbackMemory;
  /** Changes to the seeded pending review. */
  pendingReview?: (seed: PendingReview) => PendingReview;
  /** Comment writes answer 409 `comment_published` (the review was published meanwhile). */
  commentPublished?: true;
}

/** Step f1.2 with one behaviour remark over lines 90–95 (see `remarkLayoutLines`), rated LOW. */
function remarkLayoutTour(seed: Tour): Tour {
  const tour = structuredClone(seed);
  const step = tour.flows.flatMap((f) => f.steps).find((s) => s.id === "f1.2")!;
  const [first] = step.llm.annotations;
  step.llm.annotations = [
    {
      ...first!,
      type: "behaviour",
      range: { start_line: 90, end_line: 95 },
      was: "checks the wallet",
      is: "checks the core transaction",
      text: null,
    },
  ];
  if (step.llm.assessment) step.llm.assessment.risk = "low";
  return tour;
}

/** Lines 88–96 of src/rpc/client.rs: base line 99 replaced by 90, unchanged 91–92, added comments 93–94. */
function remarkLayoutLines(answer: LinesResponse): LinesResponse {
  if (answer.path !== "src/rpc/client.rs" || answer.side !== "head" || answer.from !== 88) return answer;
  return {
    ...answer,
    to: 96,
    lines: [
      "fn check(&self) -> Result<()> {",
      "    let result = self",
      "        .core_wallet",
      "        .check_core_transaction(tx)",
      "        .await;",
      "    // retry once on a timeout",
      "    // then give up",
      "    result?",
      "}",
    ],
    changed: [90, 93, 94, 95],
    context_only: [93, 94],
    deleted: [
      {
        after_line: 89,
        base_start: 99,
        lines: ["        .wallet"],
        total_lines: 1,
        replacement: { start_line: 90, end_line: 90 },
      },
    ],
    moved: [],
  };
}

/** The seed tour with review threads read partly, or not at all (no threads listed then). */
function withThreads(seed: Tour, status: ThreadsStatus): Tour {
  const tour = structuredClone(seed);
  tour.pr_context.facts.threads_status = status;
  if (status === "not_checked") {
    tour.threads = [];
    for (const step of tour.flows.flatMap((f) => f.steps)) step.facts.thread_ids = [];
  }
  return tour;
}

/** The seed tour with a binary file in Mechanical & generated (the pre-pass lists every binary file). */
function withBinaryFile(seed: Tour): Tour {
  const tour = structuredClone(seed);
  const section = tour.quiet.not_analysed.find((s) => s.category === "generated_mechanical");
  const item = {
    file: "assets/logo.png",
    side: "head" as const,
    ranges: null,
    badge: "binary_content" as const,
    rule: "binary file (git)",
  };
  if (section) {
    section.items.push(item);
    section.file_count += 1;
  } else {
    tour.quiet.not_analysed.push({ category: "generated_mechanical", file_count: 1, items: [item] });
  }
  return tour;
}

/** Facts the server could not establish: dependency files not checked, ranges too large for the LLM. */
function withUncheckedFacts(seed: Tour): Tour {
  const tour = structuredClone(seed);
  tour.pr_context.facts.counters.dependencies_changed = [];
  tour.pr_context.facts.counters.dependency_files_not_checked = ["Cargo.lock"];
  tour.pr_context.facts.project_profile = { sources: [], memcan: "not_checked" };
  tour.quiet.unexplained.push({
    file: "src/rpc/generated_codes.rs",
    side: "head",
    ranges: [{ start_line: 1, end_line: 2400 }],
    reason: "too_large_for_prompt",
  });
  return tour;
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

const partState = (index: number, at: string): AnalysisState => ({
  state: "planning",
  stage: { kind: "plan", started_at: at, part: { index, total: 23 } },
});
const waitingProgress = (basis: EstimateBasis | null, state: AnalysisState): AnalysisProgress => {
  const part = "stage" in state ? (state.stage?.part?.index ?? 6) : 6;
  const running = state.state === "running";
  return {
    started_at: new Date(Date.parse(LOADED) - 40 * 60_000).toISOString(),
    planning: { done: running ? 23 : part - 1, total: 23, state: running ? "done" : "current" },
    pr_context: { done: running ? 1 : 0, total: 1, state: running ? "done" : "pending" },
    flows: {
      done: running ? state.done : 0,
      total: running ? state.total : null,
      state: running ? "current" : "pending",
    },
    finishing: { done: 0, total: 1, state: "pending" },
    percent: basis ? 12 : null,
    estimate:
      basis && state.state !== "paused"
        ? { remaining_seconds: 2400, basis, rough: basis !== "recent_runs" }
        : null,
  };
};

const REPORTED_USAGE: UsageTotals = {
  input_tokens: 1_200_000,
  cached_input_tokens: 900_000,
  output_tokens: 45_000,
  reasoning_output_tokens: null,
  jobs: 7,
};

const SCENARIOS = {
  "show-less": { showLess: true },
  // Owner screenshot: one remark over a replaced line, unchanged lines and added comment lines that fold.
  "remark-layout": { tour: remarkLayoutTour, lines: remarkLayoutLines },
  "lazy-flows": { lazy: true, analysis: { state: "running", done: 1, total: 5 } },
  "lazy-background": {
    lazy: true,
    background: true,
    analysis: { state: "running", done: 1, total: 5, stage: flowStage(2, "2026-11-17T18:04:00Z") },
  },
  "lazy-paused": {
    lazy: true,
    analysis: {
      state: "paused",
      reason: "user",
      paused_by: "alice",
      paused_at: "2026-11-17T18:04:00Z",
      resumes_at: null,
      done: 1,
      total: 5,
    },
  },
  "lazy-limit": {
    lazy: true,
    analysis: {
      state: "paused",
      reason: "usage_limit",
      resumes_at: "2026-11-17T14:20:00Z",
      done: 1,
      total: 5,
    },
  },
  "lazy-failed": {
    lazy: true,
    analysis: { state: "failed", flow: null, message: "The repository mirror could not be updated." },
  },
  "llm-save-fails": { llmSaveFails: true },
  "many-repos": {
    manyRepos: true,
    settings: {
      ...SETTINGS,
      memcan_status: { state: "reachable", last_success_at: "2026-11-17T18:04:00.123456Z" },
      repos: Array.from({ length: 30 }, (_, i) => ({
        owner: "acme",
        name: i === 29 ? "very-long-repository-name-".repeat(4) : `repo-${i}`,
      })),
    },
  },
  "user-paused": {
    analysis: {
      state: "paused",
      reason: "user",
      paused_by: "alice",
      paused_at: "2026-11-17T18:04:00Z",
      resumes_at: null,
      done: 3,
      total: 5,
    },
    resolve: (seed) => ({
      ...seed,
      analysis: {
        ...(seedFor("default", "analysis") as AnalysisSummary),
        state: {
          state: "paused",
          reason: "user",
          paused_by: "alice",
          paused_at: "2026-11-17T18:04:00Z",
          resumes_at: null,
          done: 3,
          total: 5,
        },
      },
    }),
  },
  "usage-progress": { analysis: { state: "planning", stage: stage("plan", LOADED) }, usage: REPORTED_USAGE },
  usage: {
    analysis: { state: "finished", total: 5 },
    usage: REPORTED_USAGE,
    tour: (seed) => ({ ...seed, analysis: { ...seed.analysis, usage: REPORTED_USAGE } }),
    settings: {
      ...SETTINGS,
      llm_usage: {
        today: REPORTED_USAGE,
        last_7_days: {
          ...REPORTED_USAGE,
          input_tokens: 3_400_000,
          cached_input_tokens: 2_900_000,
          output_tokens: 120_000,
          jobs: 23,
        },
      },
    },
  },
  "large-pr": {
    resolve: (seed) => ({
      ...seed,
      analysis: null,
      commenting_available: true,
      size_warning: {
        changed_lines: 27081,
        changed_files: 371,
        estimate: { remaining_seconds: 14400, basis: "lines_only", rough: true },
      },
    }),
  },
  "large-pr-no-history": {
    resolve: (seed) => ({
      ...seed,
      analysis: null,
      commenting_available: true,
      size_warning: { changed_lines: 3000, changed_files: 24, estimate: null },
    }),
  },
  "plan-parts": {
    analysis: partState(6, new Date(Date.parse(LOADED) - 5 * 60_000).toISOString()),
    progress: (state) => waitingProgress("this_analysis", state),
    script: [[60_000, () => partState(7, now())]],
  },
  "estimate-recent": {
    analysis: { state: "running", done: 2, total: 6, stage: flowStage(3, LOADED) },
    progress: (state) => waitingProgress("recent_runs", state),
  },
  "estimate-lines": {
    analysis: partState(6, LOADED),
    progress: (state) => waitingProgress("lines_only", state),
  },
  "estimate-none": { analysis: partState(6, LOADED), progress: (state) => waitingProgress(null, state) },
  "estimate-paused": {
    analysis: {
      state: "paused",
      done: 0,
      total: null,
      resumes_at: "2026-11-17T14:20:00Z",
      stage: { kind: "plan", started_at: LOADED, part: { index: 6, total: 23 } },
    },
    progress: (state) => waitingProgress("this_analysis", state),
  },
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
    resolve: withAnalysis({
      state: "failed",
      flow: null,
      message: "The analysis stopped because of a server error. Retry the analysis.",
    }),
    analysis: {
      state: "failed",
      flow: null,
      message: "The analysis stopped because of a server error. Retry the analysis.",
    },
  },
  "existing-cancelled": { resolve: withAnalysis({ state: "cancelled" }) },
  "existing-other-language": {
    resolve: (seed) => ({
      ...seed,
      analysis: { ...analysisAt({ state: "finished", total: 5 }), language: "pl" },
    }),
  },
  "existing-paused": {
    resolve: withAnalysis({ state: "paused", resumes_at: "2026-11-17T14:20:00Z", done: 2, total: 5 }),
    analysis: { state: "paused", resumes_at: "2026-11-17T14:20:00Z", done: 2, total: 5 },
  },
  "installations-not-checked": { settings: { ...SETTINGS, app_installations: null } },
  "no-app": {
    resolve: (seed: ResolvePrResponse) => ({ ...seed, commenting_available: false }),
    settings: { ...SETTINGS, app_installations: [] },
  },
  planning: { analysis: { state: "planning", stage: stage("plan", LOADED) } },
  // A one-flow PR: planning, then the PR context, then flow 1 (no flow done yet).
  "first-flow": {
    analysis: { state: "planning", stage: stage("plan", LOADED) },
    script: [
      [RUN_STEP_MS, () => ({ state: "running", done: 0, total: 1, stage: stage("pr_context", now()) })],
      [30_000, () => ({ state: "running", done: 0, total: 1, stage: flowStage(1, now()) })],
    ],
  },
  // The plan job waits behind another analysis's job.
  waiting: { analysis: { state: "planning", stage: stage("plan", null) } },
  // One job ended, the next is not submitted yet.
  "between-jobs": { analysis: { state: "running", done: 1, total: 5 } },
  "signal-lost": {
    analysis: { state: "running", done: 2, total: 5, stage: flowStage(3, LOADED) },
    silent: true,
  },
  running: { analysis: { state: "running", done: 2, total: 5, stage: flowStage(3, LOADED) } },
  paused: {
    analysis: {
      state: "paused",
      resumes_at: "2026-11-17T14:20:00Z",
      done: 2,
      total: 5,
      stage: flowStage(3, null),
    },
  },
  "paused-unknown": {
    analysis: { state: "paused", resumes_at: null, done: 0, total: null, stage: stage("plan", null) },
  },
  failed: {
    analysis: {
      state: "failed",
      flow: { id: "f3", title: "Classify RPC errors" },
      message: "The agent's answer did not have the expected structure.",
    },
  },
  "failed-early": {
    analysis: {
      state: "failed",
      flow: null,
      message: "The repository could not be fetched. Retry the analysis.",
    },
    // Planning had not finished: the page may say the analysis failed before any flow was planned.
    progress: (state) => waitingProgress(null, state),
  },
  finished: { analysis: { state: "finished", total: 5 } },
  "llm-error": {
    settings: {
      ...SETTINGS,
      llm_status: {
        at: "2026-11-17T08:05:00Z",
        status: { state: "not_working", message: "The LLM provider's usage limit was reached." },
      },
    },
  },
  xss: { tour: withXss, questions: xssQuestions },
  "memcan-failed": { memcan: "failed" },
  "long-answer": {
    questions: () =>
      REAL_QUESTIONS.map((question) => ({
        ...question,
        answer: {
          answer: `${"longword".repeat(100)}\n\n\`\`\`text\n${"code".repeat(200)}\n\`\`\``,
          evidence: [],
        },
      })),
  },
  "repo-not-known": { repoSettings: null },
  "binary-file": { tour: withBinaryFile },
  "comment-published": { commentPublished: true },
  // Real PRs (mocks/real): their tours and questions, synthetic file text, no pending comments.
  "real-small": {
    tour: () => realTour()!,
    questions: () => REAL_QUESTIONS,
    pendingReview: (seed) => ({ ...seed, comments: [] }),
  },
  "real-tiny": {
    tour: () => realTour()!,
    questions: () => [],
    pendingReview: (seed) => ({ ...seed, comments: [] }),
  },
  "pending-incomplete": { pendingReview: (seed) => ({ ...seed, incomplete: true }) },
  "threads-partial": { tour: (seed) => withThreads(seed, "partial") },
  "threads-not-checked": { tour: (seed) => withThreads(seed, "not_checked") },
  "unchecked-facts": { tour: withUncheckedFacts },
  "llm-unknown": {
    settings: { ...SETTINGS, llm_status: null, language: "pl" },
    repoSettings: {
      ...REPO_SETTINGS,
      related_repos: [],
      project_profile: { sources: [], memcan: "checked" },
    },
  },
} satisfies Record<string, Scenario>;

/** Name of a screen scenario. */
export type ScenarioName = keyof typeof SCENARIOS;

/** All scenario names. */
export const SCENARIO_NAMES = Object.keys(SCENARIOS) as ScenarioName[];

let current: ScenarioName | null = null;

/** Selects a scenario (null = none: fixture answers and a scripted run after Start analysis). */
export function setScenario(name: ScenarioName | null): void {
  if (name !== null && !(name in SCENARIOS)) throw new Error(`unknown scenario "${String(name)}"`);
  current = name;
}

/** The real tour of a `real-*` scenario, with the mocks' analysis id; undefined for the fixture PR. */
export function realTour(): Tour | undefined {
  if (current !== "real-small" && current !== "real-tiny") return undefined;
  const tour = REAL_TOURS[current];
  return { ...tour, analysis: { ...tour.analysis, id: (seedNow("analysis") as AnalysisSummary).id } };
}

/** The selected scenario's overrides. */
export function scenario(): Scenario {
  return current === null ? {} : SCENARIOS[current];
}
