import { HttpResponse, http, type HttpResponseResolver, type PathParams } from "msw";
import { routes, type RouteName } from "../../api/routes";
import type { AnalysisSummary, ApiErrorCode, MeResponse } from "../../api/generated/types";
import { seedNow } from "../fixtures";

type Resolver = HttpResponseResolver<PathParams>;

/** An API error response as the server sends it. */
export function apiError(status: number, code: ApiErrorCode, message: string) {
  return HttpResponse.json({ code, message }, { status });
}

const verbs = { GET: http.get, POST: http.post, PUT: http.put, PATCH: http.patch, DELETE: http.delete };

/** A handler for a contract route (`{param}` becomes `:param`); `session_csrf` routes check the token. */
export function route(name: RouteName, resolver: Resolver) {
  const { method, path, auth } = routes[name];
  return verbs[method](path.replace(/\{(\w+)\}/g, ":$1"), (info) => {
    if (auth === "session_csrf") {
      const expected = (seedNow("me") as MeResponse).csrf_token;
      if (info.request.headers.get("X-CSRF-Token") !== expected) {
        return apiError(403, "csrf_rejected", "The page is out of date. Reload it and try again.");
      }
    }
    return resolver(info);
  });
}

/** Resolver of an analysis route: `not_found` unless `id` is the fixture analysis. */
export function forAnalysis(answer: Resolver): Resolver {
  return (info) => {
    if (info.params.id !== (seedNow("analysis") as AnalysisSummary).id) {
      return apiError(404, "not_found", "This analysis does not exist.");
    }
    return answer(info);
  };
}

/** Resolver answering the seed document at `path` in the selected state. */
export const seedJson = (path: string) => () => HttpResponse.json(seedNow(path) as object);
