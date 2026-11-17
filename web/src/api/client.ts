// The only fetch layer of the web app: typed calls to the routes of crates/contracts.
import { routes, type RouteName } from "./routes";
import type {
  AnalysisEvent,
  AnalysisId,
  AnalysisSummary,
  ApiErrorCode,
  AskQuestion,
  AskRequest,
  CreateAnalysisRequest,
  CreateCommentRequest,
  CreateCommentResponse,
  DiffResponse,
  Feedback,
  FeedbackId,
  FeedbackRequest,
  LinesResponse,
  MeResponse,
  PendingComment,
  PendingReview,
  RangeQuery,
  ResolvePrRequest,
  ResolvePrResponse,
  RepoRef,
  RepoSettings,
  ReviewState,
  SetLanguageRequest,
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
    const headers = new Headers({ Accept: "application/json" });
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
    if (!response.ok) throw await errorOf(response);
    if (response.status === 204) return undefined as T;
    return (await response.json()) as T;
  }

  async function getMe(): Promise<MeResponse> {
    const me = await call<MeResponse>("getMe");
    csrfToken = me.csrf_token;
    return me;
  }

  return {
    getMe,
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
    setLanguage: (body: SetLanguageRequest) => call<void>("setLanguage", { body }),
    getRepoSettings: (repo: RepoRef) =>
      call<RepoSettings>("getRepoSettings", { params: { owner: repo.owner, name: repo.name } }),
    /** Opens the analysis event stream (docs/contracts/sse-events.md); returns a function that closes it. */
    subscribeAnalysis(id: AnalysisId, handlers: AnalysisStreamHandlers): () => void {
      const source = new EventSource(baseUrl + routePath("analysisEvents", { id }));
      const onMessage = (message: MessageEvent<string>) => {
        let event: AnalysisEvent;
        try {
          event = JSON.parse(message.data) as AnalysisEvent;
        } catch {
          return;
        }
        handlers.onEvent(event);
      };
      source.addEventListener("analysis", onMessage);
      source.addEventListener("question", onMessage);
      source.onerror = () => handlers.onError(source.readyState === EventSource.CLOSED);
      return () => source.close();
    },
  };
}

/** The API client of the app (same origin). */
export type ApiClient = ReturnType<typeof createApiClient>;

/** The app's client instance. */
export const api = createApiClient();
