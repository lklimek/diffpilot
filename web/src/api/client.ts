// The only fetch layer of the web app: typed calls to the routes of crates/contracts.
import { routes, type RouteName } from "./generated/routes";
import type {
  AccessToken,
  AnalysisEvent,
  AnalysisId,
  AnalysisSummary,
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
  DiffResponse,
  Feedback,
  FeedbackId,
  FeedbackRequest,
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
  SetLanguageRequest,
  SetTimeZoneRequest,
  Settings,
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
  source.onerror = () => onError(source.readyState === EventSource.CLOSED);
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
    const url = new URL(baseUrl + routePath(name, options.params), globalThis.location?.origin);
    for (const [key, value] of Object.entries(options.query ?? {})) url.searchParams.set(key, String(value));
    const headers = new Headers({ Accept: "application/json", ...options.headers });
    if (options.body !== undefined) headers.set("Content-Type", "application/json");
    if (csrf !== null) headers.set(CSRF_HEADER, csrf);
    try {
      return await fetch(url, {
        method: route.method,
        headers,
        credentials: "same-origin",
        body: options.body === undefined ? undefined : JSON.stringify(options.body),
      });
    } catch {
      throw new ApiError(
        0,
        "internal",
        "Cannot reach the smart-review server. Check the connection and retry.",
      );
    }
  }

  async function errorOf(response: Response): Promise<ApiError> {
    const body: unknown = await response.json().catch(() => null);
    if (isErrorBody(body)) return new ApiError(response.status, body.code, body.message);
    return new ApiError(response.status, "internal", "The server gave an unexpected answer. Retry later.");
  }

  async function call<T>(name: RouteName, options: CallOptions = {}): Promise<T> {
    const needsCsrf = routes[name].auth === "session_csrf";
    if (needsCsrf && csrfToken === undefined) await getMe();
    let response = await send(name, options, needsCsrf ? (csrfToken ?? null) : null);
    if (needsCsrf && response.status === 403) {
      const error = await errorOf(response);
      if (error.code !== "csrf_rejected") throw error;
      await getMe();
      response = await send(name, options, csrfToken ?? null);
    }
    if (response.status === 503 && IDEMPOTENT_METHODS.has(routes[name].method)) {
      const error = await errorOf(response.clone());
      if (error.code !== "unavailable") throw error;
      await new Promise((resolve) => setTimeout(resolve, retryAfterMs(response)));
      response = await send(name, options, needsCsrf ? (csrfToken ?? null) : null);
    }
    if (!response.ok) throw await errorOf(response);
    if (response.status === 204) return undefined as T;
    return (await response.json()) as T;
  }

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
    /** Ends the session (`POST /auth/logout`); the CSRF token of the old session is dropped. */
    logout: async (body: LogoutRequest) => {
      await call<void>("authLogout", { body });
      csrfToken = undefined;
    },
    resolvePr: (body: ResolvePrRequest) => call<ResolvePrResponse>("resolvePr", { body }),
    createAnalysis: (body: CreateAnalysisRequest) => call<AnalysisSummary>("createAnalysis", { body }),
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
    getSettings: () => call<Settings>("getSettings"),
    setLlm: (body: import("./generated/types").ModelChoice) =>
      call<import("./generated/types").LlmSelection>("setLlm", { body }),
    requestFlow: (id: AnalysisId, flow: string) =>
      call<AnalysisSummary>("requestFlow", { params: { id, flow } }),
    setBackgroundAnalysis: (id: AnalysisId, enabled: boolean) =>
      call<AnalysisSummary>("setBackgroundAnalysis", { params: { id }, body: { enabled } }),
    pauseAnalysis: (id: AnalysisId) => call<AnalysisSummary>("pauseAnalysis", { params: { id } }),
    resumeAnalysis: (id: AnalysisId) => call<AnalysisSummary>("resumeAnalysis", { params: { id } }),
    setLanguage: (body: SetLanguageRequest) => call<void>("setLanguage", { body }),
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
    getDashboard: () => call<DashboardSummary>("getDashboard"),
    /** One page of ranked items; `etag` of the same request's previous answer makes a 304 `unchanged`. */
    listDashboardItems: (query: ItemQuery, etag: string | null = null) =>
      conditional<ItemPage>("listDashboardItems", { query: itemQueryParams(query) }, etag),
    startDashboardRefresh: () => call<RefreshStatus>("startDashboardRefresh"),
    getDashboardRefresh: (id: RefreshId) => call<RefreshStatus>("getDashboardRefresh", { params: { id } }),
    cancelDashboardRefresh: (id: RefreshId) =>
      call<RefreshStatus>("cancelDashboardRefresh", { params: { id } }),
    rankDashboardRefresh: (id: RefreshId) => call<RefreshStatus>("rankDashboardRefresh", { params: { id } }),
    setDashboardTimeZone: (body: SetTimeZoneRequest) => call<void>("setDashboardTimeZone", { body }),
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
