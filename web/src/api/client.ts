// The only fetch layer of the web app: typed calls to the routes of crates/contracts.
import { routes, type RouteName } from "./generated/routes";
import type {
  GithubWebhook,
  LlmActivity,
  PrHistory,
  PrHistoryQuery,
  RankingUsage,
  RepositoryInstallations,
  StartRefreshRequest,
  AssessmentSettings,
  AssessmentSettingsUpdate,
  AssessmentComparison,
  AssessmentAdmissionFailure,
  CredentialStatus,
  DeviceLink,
  AccessToken,
  AnalysisEvent,
  CarryOverPage,
  CarryOverQuery,
  ChannelKind,
  ChannelTestResult,
  CodeContext,
  CommentItem,
  CommentLog,
  MarkedItems,
  CommentsOverview,
  ContextQuery,
  DecideRequest,
  DecideRest,
  Decision,
  DecisionBatchRequest,
  EffectsPreview,
  EffectsPreviewRequest,
  GateMarkRequest,
  ItemActionRequest,
  ItemsQuery,
  NotificationChannel,
  NotificationSettings,
  PrPage,
  PrRef,
  PutChannelRequest,
  UserEvent,
  UserStreamEvent,
  WatchList,
  AnalysisId,
  AnalysisSummary,
  AnchoredAskRequest,
  MyAnalyses,
  MyAnalysesQuery,
  ApiErrorCode,
  AskQuestion,
  AskRequest,
  CreateAccessToken,
  CreateAnalysisRequest,
  CreateCommentRequest,
  CreateCommentResponse,
  CreatedAccessToken,
  DashboardEvent,
  DashboardSettings,
  DashboardSettingsView,
  DashboardSummary,
  DependencyView,
  DiffResponse,
  Feedback,
  FeedbackId,
  FeedbackRequest,
  FilesResponse,
  ItemPage,
  ItemQuery,
  LinesResponse,
  LogoutRequest,
  MeResponse,
  PendingComment,
  PendingReview,
  RangeQuery,
  ReasoningEffort,
  RefreshId,
  RefreshStatus,
  ResolvePrRequest,
  ResolvePrResponse,
  RepoRef,
  RepoSettings,
  ReviewState,
  SetControlledOwnersRequest,
  SetLanguageRequest,
  SetTimeZoneRequest,
  SetViewedRequest,
  Settings,
  SnoozeRequest,
  StepId,
  Tour,
  UpdateCommentRequest,
} from "./generated/types";

/** A failed API call: `status` 0 when the server was not reached; `message` is plain text for the user. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: ApiErrorCode,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

/** User-facing API error text with a plain fallback for transport and unexpected failures. */
export function errorText(error: unknown, fallback = "Something went wrong. Try again."): string {
  return error instanceof ApiError || error instanceof AssessmentAdmissionError ? error.message : fallback;
}

/** Assessment admission uses a tagged response instead of ApiError's message envelope. */
export type AssessmentAdmissionIssue =
  AssessmentAdmissionFailure | { code: "assessment_daily_limit"; resets_at?: undefined };

export class AssessmentAdmissionError extends Error {
  constructor(readonly issue: AssessmentAdmissionIssue) {
    super(
      issue.code === "credential_missing"
        ? "Link a Codex account in Settings to assess comments."
        : "Daily comment assessment limit reached.",
    );
  }

  get code() {
    return this.issue.code;
  }
}

const CSRF_HEADER = "X-CSRF-Token";
/** Methods retried once when the server answers `unavailable`; a POST is never repeated. */
const IDEMPOTENT_METHODS: ReadonlySet<string> = new Set(["GET", "PUT", "DELETE"]);
/** Longest `Retry-After` wait honoured before that one retry. */
const MAX_RETRY_AFTER_MS = 10_000;

/** The wait an `unavailable` answer asks for (`Retry-After` seconds, 1 s when absent). */
function retryAfterMs(response: Response): number {
  const seconds = Number(response.headers.get("Retry-After") ?? "1");
  return Number.isFinite(seconds) && seconds >= 0 ? Math.min(seconds * 1000, MAX_RETRY_AFTER_MS) : 1000;
}

/** Path of a route with its `{param}` placeholders filled and encoded. */
export function routePath(name: RouteName, params: Record<string, string> = {}): string {
  return routes[name].path.replace(/\{(\w+)\}/g, (_, key: string) => {
    const value = params[key];
    if (value === undefined) throw new Error(`route ${name} needs the parameter ${key}`);
    return encodeURIComponent(value);
  });
}

interface CallOptions {
  params?: Record<string, string>;
  signal?: AbortSignal;
  query?: Record<string, string | number>;
  body?: unknown;
  headers?: Record<string, string>;
}

/** A conditional read (`If-None-Match`): `unchanged` on 304, else the body with its `ETag`. */
export type Conditional<T> = { unchanged: true } | { unchanged: false; body: T; etag: string | null };

/** The defined filters of an item query as query-string values. */
const itemQueryParams = (query: ItemQuery): Record<string, string> =>
  Object.fromEntries(
    Object.entries(query)
      .filter(([, value]) => value !== undefined)
      .map(([key, value]) => [key, String(value)]),
  );

/**
 * Opens the SSE stream at `path` and passes on the JSON events of `names` whose `event` tag matches their SSE
 * name; anything else is dropped and the stream stays open. Returns the function that closes it.
 */
function openEventStream<E>(
  path: string,
  names: readonly string[],
  onEvent: (event: E) => void,
  onError: (closed: boolean) => void,
  onReconnect?: () => void,
): () => void {
  const source = new EventSource(path);
  const onMessage = (message: MessageEvent<string>) => {
    let event: unknown;
    try {
      event = JSON.parse(message.data);
    } catch {
      return;
    }
    if (
      typeof event !== "object" ||
      event === null ||
      (event as { event?: unknown }).event !== message.type
    ) {
      return;
    }
    onEvent(event as E);
  };
  // Named events only reach listeners of their name: any other type is ignored by the browser.
  for (const name of names) source.addEventListener(name, onMessage);
  let disconnected = false;
  source.onerror = () => {
    disconnected = true;
    onError(source.readyState === EventSource.CLOSED);
  };
  source.onopen = () => {
    if (disconnected) onReconnect?.();
    disconnected = false;
  };
  return () => source.close();
}

/** Callbacks of a dashboard refresh event stream (`sse-events.md` "Dashboard refresh events"). */
export interface DashboardStreamHandlers {
  /** One event: the full refresh status, or a heartbeat. */
  onEvent: (event: DashboardEvent) => void;
  /** The connection broke: `closed` when the browser gave up (HTTP error), else it is retrying. */
  onError: (closed: boolean) => void;
}

function isErrorBody(value: unknown): value is { code: ApiErrorCode; message: string } {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof (value as { code?: unknown }).code === "string" &&
    typeof (value as { message?: unknown }).message === "string"
  );
}

/** Callbacks of the per-user event stream (RW3). */
export interface UserEventHandlers {
  onEvent: (event: UserEvent) => void;
  onActivity?: (activity: LlmActivity) => void;
  onReconnect?: () => void;
  onError?: (closed: boolean) => void;
}

/** SSE names of RW3 (`sse-events.md` "Review-loop events"): the `event` tag of `UserStreamEvent`. */
const USER_STREAM_NAMES = ["user_event", "llm_activity", "heartbeat"] satisfies UserStreamEvent["event"][];

/** Route parameters of a pull request (`{owner}/{name}/{number}`). */
const prParams = (pr: PrRef) => ({ owner: pr.repo.owner, name: pr.repo.name, number: String(pr.number) });

/** Route parameters of a review-loop item: the PR and the item's per-PR number `C<no>`. */
const itemParams = (pr: PrRef, no: number) => ({ ...prParams(pr), item: `C${no}` });

/** The defined values of a query object as query-string values. */
const definedParams = (query: object): Record<string, string> =>
  Object.fromEntries(
    Object.entries(query)
      .filter(([, value]) => value !== undefined && value !== null)
      .map(([key, value]) => [key, String(value)]),
  );

/** Callbacks of an analysis event stream. */
export interface AnalysisStreamHandlers {
  /** One event (a full snapshot of the analysis or of one question). */
  onEvent: (event: AnalysisEvent) => void;
  /** The connection broke: `closed` when the browser gave up (HTTP error), else it is retrying. */
  onError: (closed: boolean) => void;
}

/** Creates a client for the API at `baseUrl` (empty = same origin); holds the session's CSRF token. */
export function createApiClient(baseUrl = "") {
  let csrfToken: string | null | undefined;

  async function send(name: RouteName, options: CallOptions, csrf: string | null): Promise<Response> {
    const route = routes[name];
    const path = routePath(name, options.params);
    const url = new URL(baseUrl + path, globalThis.location?.origin);
    for (const [key, value] of Object.entries(options.query ?? {})) url.searchParams.set(key, String(value));
    const headers = new Headers({ Accept: "application/json", ...options.headers });
    if (options.body !== undefined) headers.set("Content-Type", "application/json");
    if (csrf !== null) headers.set(CSRF_HEADER, csrf);
    try {
      return await fetch(url, {
        method: route.method,
        headers,
        credentials: "same-origin",
        signal: options.signal,
        body: options.body === undefined ? undefined : JSON.stringify(options.body),
      });
    } catch (error) {
      if (options.signal?.aborted) throw error;
      throw new ApiError(0, "internal", "Cannot reach the diffpilot server. Check the connection and retry.");
    }
  }

  async function errorOf(response: Response): Promise<ApiError | AssessmentAdmissionError> {
    const body: unknown = await response.json().catch(() => null);
    if (body && typeof body === "object" && "code" in body) {
      if (response.status === 409 && body.code === "credential_missing")
        return new AssessmentAdmissionError({ code: body.code });
      if (response.status === 429 && body.code === "assessment_daily_limit") {
        const resetsAt = "resets_at" in body ? body.resets_at : undefined;
        const resets_at =
          typeof resetsAt === "string" && Number.isFinite(Date.parse(resetsAt)) ? resetsAt : undefined;
        return new AssessmentAdmissionError({ code: body.code, resets_at });
      }
    }
    if (isErrorBody(body)) return new ApiError(response.status, body.code, body.message);
    return new ApiError(response.status, "internal", "The server gave an unexpected answer. Retry later.");
  }

  function call<T>(name: RouteName, options?: CallOptions): Promise<T>;
  function call<T>(name: RouteName, options: CallOptions, allowAbsent: true): Promise<T | null>;
  async function call<T>(name: RouteName, options: CallOptions = {}, allowAbsent = false): Promise<T | null> {
    const route = routes[name];
    const needsCsrf = route.auth === "session_csrf";
    if (needsCsrf && csrfToken === undefined) await getMe();
    let response = await send(name, options, needsCsrf ? (csrfToken ?? null) : null);
    if (needsCsrf && response.status === 403) {
      const error = await errorOf(response);
      if (error.code !== "csrf_rejected") throw error;
      await getMe();
      response = await send(name, options, csrfToken ?? null);
    }
    if (response.status === 503 && IDEMPOTENT_METHODS.has(route.method)) {
      const error = await errorOf(response.clone());
      if (error.code !== "unavailable") throw error;
      await new Promise((resolve) => setTimeout(resolve, retryAfterMs(response)));
      response = await send(name, options, needsCsrf ? (csrfToken ?? null) : null);
    }
    if (allowAbsent && response.status === 501) return null;
    if (!response.ok) throw await errorOf(response);
    // 204, and the 202 of RC3 and RC6, carry no body.
    const body = await response.text();
    return (body ? JSON.parse(body) : undefined) as T;
  }

  const optional = <T>(name: RouteName, options: CallOptions = {}) => call<T>(name, options, true);

  /** A read with `If-None-Match: etag` (none when null): 304 is `unchanged`. */
  async function conditional<T>(
    name: RouteName,
    options: CallOptions,
    etag: string | null,
  ): Promise<Conditional<T>> {
    const headers = etag === null ? undefined : { "If-None-Match": etag };
    const response = await send(name, { ...options, headers }, null);
    if (response.status === 304) return { unchanged: true };
    if (!response.ok) throw await errorOf(response);
    return { unchanged: false, body: (await response.json()) as T, etag: response.headers.get("ETag") };
  }

  async function getMe(): Promise<MeResponse> {
    const me = await call<MeResponse>("getMe");
    csrfToken = me.csrf_token;
    return me;
  }

  return {
    getMe,
    listMyPrs: (query: PrHistoryQuery, signal?: AbortSignal) =>
      optional<PrHistory>("listMyPrs", { query: definedParams(query), signal }),
    /** Ends the session (`POST /auth/logout`); the CSRF token of the old session is dropped. */
    logout: async (body: LogoutRequest) => {
      await call<void>("authLogout", { body });
      csrfToken = undefined;
    },
    resolvePr: (body: ResolvePrRequest) => call<ResolvePrResponse>("resolvePr", { body }),
    createAnalysis: (body: CreateAnalysisRequest) => call<AnalysisSummary>("createAnalysis", { body }),
    listMyAnalyses: (query: MyAnalysesQuery = {}) =>
      call<MyAnalyses>("listMyAnalyses", { query: { ...query } }),
    getAnalysis: (id: AnalysisId) => call<AnalysisSummary>("getAnalysis", { params: { id } }),
    getTour: (id: AnalysisId) => call<Tour>("getTour", { params: { id } }),
    getLines: (id: AnalysisId, query: RangeQuery) =>
      call<LinesResponse>("getLines", { params: { id }, query: { ...query } }),
    getDiff: (id: AnalysisId, query: RangeQuery) =>
      call<DiffResponse>("getDiff", { params: { id }, query: { ...query } }),
    getReviewState: (id: AnalysisId) => call<ReviewState>("getReviewState", { params: { id } }),
    putReviewState: (id: AnalysisId, body: ReviewState) =>
      call<void>("putReviewState", { params: { id }, body }),
    listQuestions: (id: AnalysisId) => call<AskQuestion[]>("listQuestions", { params: { id } }),
    askQuestion: (id: AnalysisId, stepId: StepId, body: AskRequest) =>
      call<AskQuestion>("askQuestion", { params: { id, step_id: stepId }, body }),
    askAnchored: (id: AnalysisId, body: AnchoredAskRequest) =>
      call<AskQuestion>("askAnchored", { params: { id }, body }),
    getFiles: (id: AnalysisId) => call<FilesResponse>("getFiles", { params: { id } }),
    setFileViewed: (id: AnalysisId, body: SetViewedRequest) =>
      call<void>("setFileViewed", { params: { id }, body }),
    getDependencies: (id: AnalysisId) => call<DependencyView>("getDependencies", { params: { id } }),
    listFeedback: (id: AnalysisId) => call<Feedback[]>("listFeedback", { params: { id } }),
    postFeedback: (id: AnalysisId, body: FeedbackRequest) =>
      call<Feedback>("postFeedback", { params: { id }, body }),
    deleteFeedback: (id: AnalysisId, feedbackId: FeedbackId) =>
      call<void>("deleteFeedback", { params: { id, feedback_id: feedbackId } }),
    getPendingReview: (id: AnalysisId) => call<PendingReview>("getPendingReview", { params: { id } }),
    createComment: (id: AnalysisId, body: CreateCommentRequest) =>
      call<CreateCommentResponse>("createComment", { params: { id }, body }),
    updateComment: (id: AnalysisId, commentId: string, body: UpdateCommentRequest) =>
      call<PendingComment>("updateComment", { params: { id, comment_id: commentId }, body }),
    deleteComment: (id: AnalysisId, commentId: string) =>
      call<void>("deleteComment", { params: { id, comment_id: commentId } }),
    cancelAnalysis: (id: AnalysisId) => call<AnalysisSummary>("cancelAnalysis", { params: { id } }),
    retryAnalysis: (id: AnalysisId) => call<AnalysisSummary>("retryAnalysis", { params: { id } }),
    getCodex: () => call<CredentialStatus>("getCodex"),
    getCodexLink: () => call<CredentialStatus>("getCodexLink"),
    linkCodex: () => call<DeviceLink>("linkCodex", { body: { mode: "chatgpt_plan" } }),
    unlinkCodex: () => call<void>("deleteCodex"),
    getAssessmentSettings: () => call<AssessmentSettings>("getAssessmentSettings"),
    setAssessmentSettings: (body: AssessmentSettingsUpdate) =>
      call<AssessmentSettings>("setAssessmentSettings", { body }),
    getAssessmentComparison: async (pr: PrRef) => {
      try {
        return await call<AssessmentComparison>("getAssessmentComparison", { params: prParams(pr) });
      } catch (error) {
        if (error instanceof ApiError && error.status === 404) return null;
        throw error;
      }
    },
    getSettings: () => call<Settings>("getSettings"),
    getGithubWebhook: (signal?: AbortSignal) => optional<GithubWebhook>("getGithubWebhook", { signal }),
    getRepositoryInstallations: (signal?: AbortSignal) =>
      optional<RepositoryInstallations>("getRepositoryInstallations", { signal }),
    getLlmActivity: (signal?: AbortSignal) => optional<LlmActivity>("getLlmActivity", { signal }),
    getRankingUsage: (signal?: AbortSignal) => optional<RankingUsage>("getRankingUsage", { signal }),
    resetRankingUsage: () => call<RankingUsage>("resetRankingUsage"),
    setLlm: (body: import("./generated/types").ModelChoice) =>
      call<import("./generated/types").LlmSelection>("setLlm", { body }),
    requestFlow: (id: AnalysisId, flow: string) =>
      call<AnalysisSummary>("requestFlow", { params: { id, flow } }),
    setBackgroundAnalysis: (id: AnalysisId, enabled: boolean) =>
      call<AnalysisSummary>("setBackgroundAnalysis", { params: { id }, body: { enabled } }),
    pauseAnalysis: (id: AnalysisId) => call<AnalysisSummary>("pauseAnalysis", { params: { id } }),
    resumeAnalysis: (id: AnalysisId) => call<AnalysisSummary>("resumeAnalysis", { params: { id } }),
    setLanguage: (body: SetLanguageRequest) => call<void>("setLanguage", { body }),
    setControlledOwners: (body: SetControlledOwnersRequest) =>
      call<import("./generated/types").ControlledOwners>("setControlledOwners", { body }),
    getRepoSettings: (repo: RepoRef) =>
      call<RepoSettings>("getRepoSettings", { params: { owner: repo.owner, name: repo.name } }),
    getDashboardSettings: () => call<DashboardSettingsView>("getDashboardSettings"),
    setDashboardSettings: (body: DashboardSettings) =>
      call<DashboardSettingsView>("setDashboardSettings", { body }),
    setAskEffort: (reasoning_effort: ReasoningEffort) =>
      call<import("./generated/types").LlmSelection>("setAskEffort", { body: { reasoning_effort } }),
    setJudgeEffort: (reasoning_effort: ReasoningEffort) =>
      call<import("./generated/types").LlmSelection>("setJudgeEffort", { body: { reasoning_effort } }),
    listAccessTokens: () => call<AccessToken[]>("listAccessTokens"),
    /** The only call whose answer carries a token secret (ADR-019); callers must not cache or store it. */
    createAccessToken: (body: CreateAccessToken) => call<CreatedAccessToken>("createAccessToken", { body }),
    revokeAccessToken: (id: string) => call<void>("revokeAccessToken", { params: { id } }),
    /** Opens the analysis event stream (docs/contracts/sse-events.md); returns a function that closes it. */
    subscribeAnalysis(id: AnalysisId, handlers: AnalysisStreamHandlers): () => void {
      return openEventStream<AnalysisEvent>(
        baseUrl + routePath("analysisEvents", { id }),
        ["analysis", "question", "heartbeat"] satisfies AnalysisEvent["event"][],
        handlers.onEvent,
        handlers.onError,
      );
    },
    /** The summary with the excluded items listed (`?excluded=true`, B4). */
    getDashboard: () => call<DashboardSummary>("getDashboard", { query: { excluded: "true" } }),
    /** One page of ranked items; `etag` of the same request's previous answer makes a 304 `unchanged`. */
    listDashboardItems: (query: ItemQuery, etag: string | null = null) =>
      conditional<ItemPage>("listDashboardItems", { query: itemQueryParams(query) }, etag),
    hideDashboardMention: (id: string) => call<void>("hideDashboardMention", { params: { id } }),
    /** Snoozes one ranked item for `days` (1–365, R12); a later snooze replaces it. */
    snoozeDashboardItem: (key: string, days: number) =>
      call<void>("snoozeDashboardItem", { params: { key }, body: { days } satisfies SnoozeRequest }),
    /** Ends an item's snooze now ("Wake now", R13). */
    wakeDashboardItem: (key: string) => call<void>("wakeDashboardItem", { params: { key } }),
    startDashboardRefresh: (body?: StartRefreshRequest) =>
      call<RefreshStatus>("startDashboardRefresh", { body }),
    rejudgeDashboardItem: (key: string) => call<void>("rejudgeDashboardItem", { params: { key } }),
    getDashboardRefresh: (id: RefreshId) => call<RefreshStatus>("getDashboardRefresh", { params: { id } }),
    cancelDashboardRefresh: (id: RefreshId) =>
      call<RefreshStatus>("cancelDashboardRefresh", { params: { id } }),
    rankDashboardRefresh: (id: RefreshId) => call<RefreshStatus>("rankDashboardRefresh", { params: { id } }),
    setDashboardTimeZone: (body: SetTimeZoneRequest) => call<void>("setDashboardTimeZone", { body }),
    getPrPage: (pr: PrRef) => call<PrPage>("getPrPage", { params: prParams(pr) }),
    getComments: (pr: PrRef) => call<CommentsOverview>("getComments", { params: prParams(pr) }),
    visitComments: (pr: PrRef) => optional<void>("visitComments", { params: prParams(pr) }),
    startCommentAssessment: (pr: PrRef, signal?: AbortSignal) =>
      optional<void>("startCommentAssessment", { params: prParams(pr), signal }),
    readComments: (pr: PrRef) => call<void>("readComments", { params: prParams(pr) }),
    listCommentItems: (pr: PrRef, query: ItemsQuery = { state: null, kind: null }) =>
      call<CommentItem[]>("listCommentItems", { params: prParams(pr), query: definedParams(query) }),
    getCommentContext: (pr: PrRef, no: number, query: ContextQuery = { from: null, to: null }) =>
      call<CodeContext>("getCommentContext", { params: itemParams(pr, no), query: definedParams(query) }),
    assessComment: (pr: PrRef, no: number) => call<void>("assessComment", { params: itemParams(pr, no) }),
    decideComment: (pr: PrRef, no: number, body: DecideRequest) =>
      call<Decision>("decideComment", { params: itemParams(pr, no), body }),
    undoCommentDecision: (pr: PrRef, no: number, body: ItemActionRequest) =>
      call<CommentItem>("undoCommentDecision", { params: itemParams(pr, no), body }),
    reopenCommentItem: (pr: PrRef, no: number, body: ItemActionRequest) =>
      call<CommentItem>("reopenCommentItem", { params: itemParams(pr, no), body }),
    skipCommentItem: (pr: PrRef, no: number, body: ItemActionRequest) =>
      call<void>("skipCommentItem", { params: itemParams(pr, no), body }),
    markCommentGate: (pr: PrRef, no: number, body: GateMarkRequest) =>
      call<CommentItem>("markCommentGate", { params: itemParams(pr, no), body }),
    /** Asks about a review-comment item; with an analysis of the head its Ask session answers, else a question pass. */
    askPrQuestion: (pr: PrRef, body: AnchoredAskRequest) =>
      call<AskQuestion>("askPrQuestion", { params: prParams(pr), body }),
    /** The reader's own questions about the PR's review-comment items, oldest first. */
    listPrQuestions: (pr: PrRef) => call<AskQuestion[]>("listPrQuestions", { params: prParams(pr) }),
    decideCommentBatch: (pr: PrRef, body: DecisionBatchRequest) =>
      call<Decision[]>("decideCommentBatch", { params: prParams(pr), body }),
    /** The effects the server plans for the rows, with the reply texts it would post; nothing is stored (RC16). */
    previewCommentEffects: (pr: PrRef, body: EffectsPreviewRequest) =>
      call<EffectsPreview[]>("previewCommentEffects", { params: prParams(pr), body }),
    /** The server's "Decide the rest per recommendation" list and the items it leaves out (RC17). */
    getDecideRest: (pr: PrRef) => call<DecideRest>("getDecideRest", { params: prParams(pr) }),
    getCommentLog: (pr: PrRef) => call<CommentLog>("getCommentLog", { params: prParams(pr) }),
    listMarkedItems: (cursor: string | null) =>
      call<MarkedItems>("listMarkedItems", { query: definedParams({ cursor }) }),
    listCarryOvers: (query: CarryOverQuery) =>
      call<CarryOverPage>("listCarryOvers", { query: definedParams(query) }),
    watchPr: (pr: PrRef) => call<void>("watchPr", { params: prParams(pr) }),
    unwatchPr: (pr: PrRef) => call<void>("unwatchPr", { params: prParams(pr) }),
    listWatches: () => call<WatchList>("listWatches"),
    getNotificationSettings: () => call<NotificationSettings>("getNotificationSettings"),
    putNotificationChannel: (kind: ChannelKind, body: PutChannelRequest) =>
      call<NotificationChannel>("putNotificationChannel", { params: { kind }, body }),
    deleteNotificationChannel: (kind: ChannelKind) =>
      call<void>("deleteNotificationChannel", { params: { kind } }),
    testNotificationChannel: (kind: ChannelKind) =>
      call<ChannelTestResult>("testNotificationChannel", { params: { kind } }),
    /** Opens the per-user event stream (RW3); returns a function that closes it. */
    subscribeUserEvents(handlers: UserEventHandlers): () => void {
      return openEventStream<UserStreamEvent>(
        baseUrl + routePath("userEvents"),
        USER_STREAM_NAMES,
        (event) => {
          if (event.event === "user_event") handlers.onEvent(event.user_event);
          if (event.event === "llm_activity") handlers.onActivity?.(event.activity);
        },
        handlers.onError ?? (() => {}),
        handlers.onReconnect,
      );
    },
    /** Opens a refresh's event stream; returns a function that closes it. */
    subscribeDashboardRefresh(id: RefreshId, handlers: DashboardStreamHandlers): () => void {
      return openEventStream<DashboardEvent>(
        baseUrl + routePath("dashboardRefreshEvents", { id }),
        ["refresh", "heartbeat"] satisfies DashboardEvent["event"][],
        handlers.onEvent,
        handlers.onError,
      );
    },
  };
}

/** The API client of the app (same origin). */
export type ApiClient = ReturnType<typeof createApiClient>;

/** The app's client instance. */
export const api = createApiClient();
