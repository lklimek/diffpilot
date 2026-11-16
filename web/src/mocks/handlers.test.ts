import { setupServer } from "msw/node";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { ApiError, createApiClient } from "../api/client";
import { routes } from "../api/generated/routes";
import { setFixtureState } from "./fixtures";
import { handlers } from "./handlers";

const BASE = location.origin;
const server = setupServer(...handlers);
beforeAll(() => server.listen({ onUnhandledFrame: "error" }));
afterEach(() => setFixtureState("default"));
afterAll(() => server.close());

const ID = "6d3a2f1e-0482-4c5e-9a7b-000000000482";

describe("msw handlers from fixtures/pr-482", () => {
  it("cover the routes the slice uses", () => {
    const covered = handlers.map((h) => h.info.header);
    for (const name of [
      "getMe",
      "resolvePr",
      "createAnalysis",
      "getAnalysis",
      "getTour",
      "getLines",
    ] as const) {
      const path = routes[name].path.replace(/\{(\w+)\}/g, ":$1");
      expect(covered).toContain(`${routes[name].method} ${path}`);
    }
  });

  it("answer the slice flow: resolve, create, tour", async () => {
    const api = createApiClient(BASE);
    const resolved = await api.resolvePr({ url: "https://github.com/acme/node-client/pull/482" });
    expect(resolved.analysis).toBeNull();
    const created = await api.createAnalysis({
      pr: { repo: resolved.pr.repo, number: 482 },
      head_sha: resolved.pr.head_sha,
    });
    expect(created.state).toEqual({ state: "finished", total: 5 });
    const tour = await api.getTour(created.id);
    expect(tour.flows.map((f) => f.id)).toEqual(["f1", "f2", "f3", "f4", "f5"]);
  });

  it("reject a mutating call without the fixture CSRF token", async () => {
    const response = await fetch(`${BASE}/api/v1/pr/resolve`, { method: "POST", body: "{}" });
    expect(response.status).toBe(403);
    expect(await response.json()).toMatchObject({ code: "csrf_rejected" });
  });

  it("serve the selected fixture state", async () => {
    setFixtureState("resume");
    const api = createApiClient(BASE);
    expect((await api.getReviewState(ID)).ticked_steps).toEqual(["f2.1"]);
    expect((await api.resolvePr({ url: "https://github.com/acme/node-client/pull/482" })).analysis?.id).toBe(
      ID,
    );
  });

  it("answer not_found for another analysis id", async () => {
    const error = await createApiClient(BASE)
      .getTour("00000000-0000-0000-0000-000000000000")
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ status: 404, code: "not_found" });
  });

  it("slice lines to the requested range with the gutter facts inside it", async () => {
    const lines = await createApiClient(BASE).getLines(ID, {
      path: "src/rpc/client.rs",
      side: "head",
      from: 88,
      to: 96,
    });
    expect(lines).toMatchObject({ from: 88, to: 96, total_lines: 224 });
    expect(lines.lines).toHaveLength(9);
    expect(lines.lines[0]).toContain("impl RpcClient");
    expect(lines.changed).toEqual([89, 91, 92, 93, 94, 95, 96]);
    expect(lines.deleted.map((d) => d.after_line)).toEqual([88, 90]);
  });

  it("clamp the end of a range to the file", async () => {
    const lines = await createApiClient(BASE).getLines(ID, {
      path: "src/rpc/client.rs",
      side: "head",
      from: 220,
      to: 400,
    });
    expect(lines.to).toBe(224);
    expect(lines.lines).toHaveLength(5);
  });

  it("reject ranges over 2000 lines, inverted ranges and unknown files", async () => {
    const api = createApiClient(BASE);
    const ask = (from: number, to: number, path = "src/rpc/client.rs") =>
      api.getLines(ID, { path, side: "head", from, to }).catch((e: unknown) => e);
    expect(await ask(1, 2001)).toMatchObject({ status: 400, code: "range_too_large" });
    expect(await ask(9, 3)).toMatchObject({ status: 400, code: "invalid_request" });
    expect(await ask(1, 5, "src/nope.rs")).toMatchObject({ status: 404, code: "not_found" });
  });
});
