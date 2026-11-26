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

  it("names a review-loop item by its per-PR number C<no> in the path, as the server reads it", async () => {
    const paths: string[] = [];
    server.use(
      me("tok-1"),
      http.post(`${BASE}/api/v1/prs/acme/node-client/482/comments/items/:item/:action`, ({ request }) => {
        paths.push(new URL(request.url).pathname);
        return HttpResponse.json({});
      }),
      http.get(`${BASE}/api/v1/prs/acme/node-client/482/comments/items/:item/context`, ({ request }) => {
        paths.push(new URL(request.url).pathname);
        return HttpResponse.json({});
      }),
    );
    const api = createApiClient(BASE);
    const pr = { repo: { owner: "acme", name: "node-client" }, number: 482 };
    await api.getCommentContext(pr, 14);
    await api.undoCommentDecision(pr, 14, { item_revision: 3 });
    await api.reopenCommentItem(pr, 15, { item_revision: 1 });
    await api.markCommentGate(pr, 16, { gate: "G-FUNDS", marked: true, item_revision: 2 });
    expect(paths.map((path) => path.replace("/api/v1/prs/acme/node-client/482/comments/items/", ""))).toEqual(
      ["C14/context", "C14/undo", "C15/reopen", "C16/gates"],
    );
  });

  it("a 202 Accepted without a body is a success (RC3 read again, RC6 assess again)", async () => {
    server.use(
      me("tok-1"),
      http.post(
        `${BASE}/api/v1/prs/acme/node-client/482/comments/read`,
        () => new HttpResponse(null, { status: 202 }),
      ),
      http.post(
        `${BASE}/api/v1/prs/acme/node-client/482/comments/items/:item/assess`,
        () => new HttpResponse(null, { status: 202 }),
      ),
    );
    const api = createApiClient(BASE);
    const pr = { repo: { owner: "acme", name: "node-client" }, number: 482 };
    await expect(api.readComments(pr)).resolves.toBeUndefined();
    await expect(api.assessComment(pr, 14)).resolves.toBeUndefined();
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

  it.each([
    ["llm", (api, scope) => api.setLlm({ model: "gpt-6-sol", reasoning_effort: "low" }, scope)],
    ["llm/ask-effort", (api, scope) => api.setAskEffort("low", scope)],
    ["llm/judge-effort", (api, scope) => api.setJudgeEffort("low", scope)],
    ["llm/job-slots", (api, scope) => api.setLlmJobSlots(3, scope)],
  ] satisfies [string, (api: ReturnType<typeof createApiClient>, scope: number) => Promise<unknown>][])(
    "keeps an open %s form bound to its original session",
    async (route, save) => {
      let actor = { login: "alice", token: "alice-session" };
      const seen: (string | null)[] = [];
      server.use(
        http.get(`${BASE}/api/v1/me`, () =>
          HttpResponse.json({
            user: { login: actor.login, name: null, avatar_url: null, language: "en" },
            csrf_token: actor.token,
          }),
        ),
        http.put(`${BASE}/api/v1/settings/${route}`, ({ request }) => {
          const token = request.headers.get("X-CSRF-Token");
          seen.push(token);
          if (token !== actor.token) {
            return HttpResponse.json({ code: "csrf_rejected", message: "Reload." }, { status: 403 });
          }
          return HttpResponse.json({ saved: true });
        }),
      );
      const api = createApiClient(BASE);
      const { sessionScope } = await api.getBrowserSession();
      expect((await api.getBrowserSession()).sessionScope).toBe(sessionScope);

      actor = { login: "bob", token: "bob-session" };
      await expect(save(api, sessionScope)).rejects.toMatchObject({ code: "csrf_rejected" });
      await expect(save(api, sessionScope)).rejects.toMatchObject({ code: "csrf_rejected" });
      expect(seen).toEqual(["alice-session"]);

      const current = await api.getBrowserSession();
      expect(current.sessionScope).not.toBe(sessionScope);
      await expect(save(api, current.sessionScope)).resolves.toEqual({ saved: true });
      expect(seen).toEqual(["alice-session", "bob-session"]);
    },
  );

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

  describe("a busy server (503 unavailable)", () => {
    const BUSY = "The server is busy. Try again in a moment.";
    /** Answers `unavailable` with `retryAfter` the first `busyTimes` calls, then `ok`; counts calls. */
    const busyThen = (busyTimes: number, retryAfter: string, ok: () => Response) => {
      const calls: number[] = [];
      const respond = () => {
        calls.push(Date.now());
        if (calls.length <= busyTimes) {
          return HttpResponse.json(
            { code: "unavailable", message: BUSY },
            { status: 503, headers: { "Retry-After": retryAfter } },
          );
        }
        return ok();
      };
      return { calls, respond };
    };

    it("retries an idempotent GET once after the Retry-After delay", async () => {
      const busy = busyThen(1, "1", () => HttpResponse.json({ ticked_steps: [], last_viewed: null }));
      server.use(http.get(`${BASE}/api/v1/analyses/:id/review-state`, busy.respond));
      await expect(createApiClient(BASE).getReviewState("a1")).resolves.toEqual({
        ticked_steps: [],
        last_viewed: null,
      });
      expect(busy.calls).toHaveLength(2);
      const [first = 0, second = 0] = busy.calls;
      expect(second - first).toBeGreaterThanOrEqual(900);
    });

    it("retries PUT and DELETE once", async () => {
      const put = busyThen(1, "0", () => new HttpResponse(null, { status: 204 }));
      const del = busyThen(1, "0", () => new HttpResponse(null, { status: 204 }));
      server.use(
        me("tok-1"),
        http.put(`${BASE}/api/v1/analyses/:id/review-state`, put.respond),
        http.delete(`${BASE}/api/v1/analyses/:id/feedback/:feedback_id`, del.respond),
      );
      const api = createApiClient(BASE);
      await api.putReviewState("a1", { ticked_steps: [], last_viewed: null });
      await api.deleteFeedback("a1", "fb-1");
      expect(put.calls).toHaveLength(2);
      expect(del.calls).toHaveLength(2);
    });

    it("gives up after one retry with the server's sentence", async () => {
      const busy = busyThen(2, "0", () => HttpResponse.json({}));
      server.use(http.get(`${BASE}/api/v1/analyses/:id/tour`, busy.respond));
      const error = await createApiClient(BASE)
        .getTour("a1")
        .catch((e: unknown) => e);
      expect(error).toMatchObject({ status: 503, code: "unavailable", message: BUSY });
      expect(busy.calls).toHaveLength(2);
    });

    it("never retries a POST and shows the server's sentence", async () => {
      const busy = busyThen(1, "0", () => HttpResponse.json({}));
      server.use(me("tok-1"), http.post(`${BASE}/api/v1/pr/resolve`, busy.respond));
      const error = await createApiClient(BASE)
        .resolvePr({ url: "https://github.com/acme/node-client/pull/482" })
        .catch((e: unknown) => e);
      expect(error).toBeInstanceOf(ApiError);
      expect(error).toMatchObject({ status: 503, code: "unavailable", message: BUSY });
      expect(busy.calls).toHaveLength(1);
    });
  });
});
