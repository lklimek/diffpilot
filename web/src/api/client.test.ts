// @vitest-environment node
import { http, HttpResponse } from "msw";
import { setupServer } from "msw/node";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { ApiError, createApiClient, routePath } from "./client";

const BASE = "http://api.test";
const server = setupServer();
beforeAll(() => server.listen({ onUnhandledFrame: "error" }));
afterEach(() => server.resetHandlers());
afterAll(() => server.close());

const me = (token: string | null) =>
  http.get(`${BASE}/api/v1/me`, () =>
    HttpResponse.json({
      user: token ? { login: "alice", name: null, avatar_url: null, language: "en" } : null,
      csrf_token: token,
    }),
  );

describe("routePath", () => {
  it("fills and encodes path parameters", () => {
    expect(routePath("askQuestion", { id: "a 1", step_id: "f1.2" })).toBe(
      "/api/v1/analyses/a%201/steps/f1.2/questions",
    );
  });

  it("rejects a missing parameter", () => {
    expect(() => routePath("getTour", {})).toThrow(/id/);
  });
});

describe("api client", () => {
  it("sends the CSRF token from /api/v1/me on mutating routes", async () => {
    let header: string | null = "unset";
    server.use(
      me("tok-1"),
      http.post(`${BASE}/api/v1/pr/resolve`, ({ request }) => {
        header = request.headers.get("X-CSRF-Token");
        return HttpResponse.json({ pr: {}, commenting_available: true, analysis: null, head_moved: false });
      }),
    );
    const api = createApiClient(BASE);
    await api.resolvePr({ url: "https://github.com/acme/node-client/pull/482" });
    expect(header).toBe("tok-1");
  });

  it("removes a feedback value with DELETE …/feedback/{feedback_id} and the CSRF token", async () => {
    let seen: { method: string; path: string; csrf: string | null } | null = null;
    server.use(
      me("tok-1"),
      http.delete(`${BASE}/api/v1/analyses/:id/feedback/:feedback_id`, ({ request }) => {
        seen = {
          method: request.method,
          path: new URL(request.url).pathname,
          csrf: request.headers.get("X-CSRF-Token"),
        };
        return new HttpResponse(null, { status: 204 });
      }),
    );
    await expect(createApiClient(BASE).deleteFeedback("a1", "fb 1")).resolves.toBeUndefined();
    expect(seen).toEqual({ method: "DELETE", path: "/api/v1/analyses/a1/feedback/fb%201", csrf: "tok-1" });
  });

  it("does not send the CSRF header on GET routes", async () => {
    let header: string | null = "unset";
    server.use(
      http.get(`${BASE}/api/v1/analyses/:id/tour`, ({ request }) => {
        header = request.headers.get("X-CSRF-Token");
        return HttpResponse.json({});
      }),
    );
    await createApiClient(BASE).getTour("a1");
    expect(header).toBeNull();
  });

  it("refreshes the CSRF token once when the server rejects it", async () => {
    const seen: (string | null)[] = [];
    let token = "old";
    server.use(
      http.get(`${BASE}/api/v1/me`, () => HttpResponse.json({ user: null, csrf_token: token })),
      http.post(`${BASE}/api/v1/analyses`, ({ request }) => {
        seen.push(request.headers.get("X-CSRF-Token"));
        if (request.headers.get("X-CSRF-Token") !== "new") {
          token = "new";
          return HttpResponse.json({ code: "csrf_rejected", message: "Reload the page." }, { status: 403 });
        }
        return HttpResponse.json({ id: "a1" }, { status: 201 });
      }),
    );
    const result = await createApiClient(BASE).createAnalysis({
      pr: { repo: { owner: "acme", name: "node-client" }, number: 482 },
      head_sha: "0".repeat(40),
    });
    expect(result.id).toBe("a1");
    expect(seen).toEqual(["old", "new"]);
  });

  it("passes query parameters of range routes", async () => {
    let url = "";
    server.use(
      http.get(`${BASE}/api/v1/analyses/:id/lines`, ({ request }) => {
        url = request.url;
        return HttpResponse.json({});
      }),
    );
    await createApiClient(BASE).getLines("a1", { path: "src/a b.rs", side: "head", from: 3, to: 9 });
    const query = new URL(url).searchParams;
    expect([query.get("path"), query.get("side"), query.get("from"), query.get("to")]).toEqual([
      "src/a b.rs",
      "head",
      "3",
      "9",
    ]);
  });

  it("turns an error body into an ApiError with code, status and plain message", async () => {
    server.use(
      http.get(`${BASE}/api/v1/analyses/:id/tour`, () =>
        HttpResponse.json(
          { code: "analysis_not_finished", message: "The analysis has not finished." },
          { status: 409 },
        ),
      ),
    );
    const error = await createApiClient(BASE)
      .getTour("a1")
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({
      status: 409,
      code: "analysis_not_finished",
      message: "The analysis has not finished.",
    });
  });

  it("reports a non-JSON error as internal without leaking the body", async () => {
    server.use(
      http.get(`${BASE}/api/v1/me`, () => new HttpResponse("<html>stack trace</html>", { status: 502 })),
    );
    const error = await createApiClient(BASE)
      .getMe()
      .catch((e: unknown) => e);
    expect(error).toMatchObject({ status: 502, code: "internal" });
    expect((error as ApiError).message).not.toContain("stack trace");
  });

  it("reports an unreachable server as an ApiError", async () => {
    server.use(http.get(`${BASE}/api/v1/me`, () => HttpResponse.error()));
    const error = await createApiClient(BASE)
      .getMe()
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ status: 0, code: "internal" });
  });
});
