import { setupServer } from "msw/node";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { ApiError, createApiClient } from "../api/client";
import { routes } from "../api/generated/routes";
import type { AnalysisEvent } from "../api/generated/types";
import { setFixtureState } from "./fixtures";
import { handlers } from "./handlers";
import { setMockLanguage } from "./handlers/get-settings";
import { RUN_STEP_MS, resetLive, setLiveState } from "./live-analysis";
import { MEMCAN_SYNC_MS, resetStore } from "./review-store";
import { FLOW_TITLES, MOVED_HEAD, setScenario } from "./scenarios";

const BASE = location.origin;
const server = setupServer(...handlers);
beforeAll(() => server.listen({ onUnhandledFrame: "error" }));
afterEach(() => {
  setFixtureState("default");
  setScenario(null);
  setMockLanguage(null);
  resetLive();
  resetStore();
  vi.useRealTimers();
});
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
      "analysisEvents",
      "cancelAnalysis",
      "retryAnalysis",
      "getSettings",
      "setLanguage",
      "getRepoSettings",
    ] as const) {
      const path = routes[name].path.replace(/\{(\w+)\}/g, ":$1");
      expect(covered).toContain(`${routes[name].method} ${path}`);
    }
  });

  it("remove one feedback value of the analysis; an unknown id is not_found", async () => {
    const api = createApiClient(BASE);
    const created = await api.postFeedback(ID, {
      target: { target: "step", step_id: "f1.5" },
      value: "wrong",
      note: null,
    });
    await api.postFeedback(ID, {
      target: { target: "step", step_id: "f1.5" },
      value: "irrelevant",
      note: null,
    });
    await api.deleteFeedback(ID, created.id);
    const left = (await api.listFeedback(ID)).filter(
      (f) => JSON.stringify(f.target) === '{"target":"step","step_id":"f1.5"}',
    );
    expect(left.map((f) => f.value)).toEqual(["irrelevant"]);
    await expect(api.deleteFeedback(ID, created.id)).rejects.toMatchObject({
      status: 404,
      code: "not_found",
    });
  });

  it.each([
    [null, "written"],
    ["memcan-failed", "failed"],
  ] as const)(
    "move a new feedback value from pending to its MemCan outcome (scenario %s)",
    async (name, outcome) => {
      setScenario(name);
      vi.useFakeTimers({ toFake: ["Date"] });
      const api = createApiClient(BASE);
      const created = await api.postFeedback(ID, {
        target: { target: "step", step_id: "f1.5" },
        value: "correct",
        note: null,
      });
      expect(created.memory).toBe("pending");
      const memoryOf = async () => (await api.listFeedback(ID)).find((f) => f.id === created.id)!.memory;
      expect(await memoryOf()).toBe("pending");
      vi.setSystemTime(Date.now() + MEMCAN_SYNC_MS);
      expect(await memoryOf()).toBe(outcome);
    },
  );

  it("serve the thread, pending-review and comment_published scenarios", async () => {
    const api = createApiClient(BASE);
    setScenario("threads-not-checked");
    const unchecked = await api.getTour(ID);
    expect(unchecked.pr_context.facts.threads_status).toBe("not_checked");
    expect(unchecked.threads).toEqual([]);
    setScenario("threads-partial");
    expect((await api.getTour(ID)).pr_context.facts.threads_status).toBe("partial");
    resetStore();
    setScenario("pending-incomplete");
    expect((await api.getPendingReview(ID)).incomplete).toBe(true);
    setScenario("comment-published");
    await expect(
      api.createComment(ID, {
        idempotency_key: crypto.randomUUID(),
        target: { path: "src/rpc/client.rs", side: "head", start_line: null, line: 96 },
        body: "x",
      }),
    ).rejects.toMatchObject({ status: 409, code: "comment_published" });
  });

  it("answer the slice flow: resolve, create, tour", async () => {
    const api = createApiClient(BASE);
    const resolved = await api.resolvePr({ url: "https://github.com/acme/node-client/pull/482" });
    expect(resolved.analysis).toBeNull();
    const created = await api.createAnalysis({
      pr: { repo: resolved.pr.repo, number: 482 },
      head_sha: resolved.pr.head_sha,
    });
    expect(created.state).toEqual({ state: "planning" });
    setLiveState({ state: "finished", total: 5 });
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

const PR_URL = "https://github.com/acme/node-client/pull/482";

/** Opens the analysis event stream; `next` reads one SSE event. */
async function openEvents() {
  const controller = new AbortController();
  const response = await fetch(`${BASE}/api/v1/analyses/${ID}/events`, { signal: controller.signal });
  expect(response.headers.get("Content-Type")).toBe("text/event-stream");
  const reader = response.body!.pipeThrough(new TextDecoderStream()).getReader();
  let text = "";
  const next = async (): Promise<AnalysisEvent> => {
    let end: number;
    while ((end = text.indexOf("\n\n")) < 0) {
      const { value, done } = await reader.read();
      if (done) throw new Error("the stream ended");
      text += value;
    }
    const frame = text.slice(0, end);
    text = text.slice(end + 2);
    const event = JSON.parse(/^data: (.*)$/m.exec(frame)![1]!) as AnalysisEvent;
    expect(event.event).toBe(/^event: (.*)$/m.exec(frame)?.[1]);
    return event;
  };
  const take = async (count: number) => {
    const events: AnalysisEvent[] = [];
    while (events.length < count) events.push(await next());
    return events;
  };
  return { take, close: () => controller.abort() };
}

const states = (events: AnalysisEvent[]) =>
  events.flatMap((e) => (e.event === "analysis" ? [e.summary.state] : []));

describe("msw handlers of the screens", () => {
  it("resolve: not a PR URL, private repository, unknown PR", async () => {
    const api = createApiClient(BASE);
    const ask = (url: string) => api.resolvePr({ url }).catch((e: unknown) => e);
    expect(await ask("https://github.com/acme/node-client/issues/5")).toMatchObject({
      status: 400,
      code: "not_a_pr_url",
    });
    expect(await ask("https://github.com/acme/private-repo/pull/1")).toMatchObject({
      status: 422,
      code: "private_repository",
    });
    expect(await ask("https://github.com/acme/node-client/pull/9")).toMatchObject({
      status: 404,
      code: "not_found",
    });
  });

  it("resolve follows the scenario (head moved, no App)", async () => {
    const api = createApiClient(BASE);
    setScenario("head-moved");
    const moved = await api.resolvePr({ url: PR_URL });
    expect(moved).toMatchObject({ head_moved: true, pr: { head_sha: MOVED_HEAD } });
    expect(moved.analysis?.pr.head_sha).not.toBe(MOVED_HEAD);
    setScenario("no-app");
    expect((await api.resolvePr({ url: PR_URL })).commenting_available).toBe(false);
  });

  it("me: nobody signed in in scenario signed-out", async () => {
    setScenario("signed-out");
    expect(await createApiClient(BASE).getMe()).toEqual({ user: null, csrf_token: null });
  });

  it(
    "events: snapshot (analysis, then every question), then the scripted run to finished",
    async () => {
      setFixtureState("resume");
      const api = createApiClient(BASE);
      await api.createAnalysis({
        pr: { repo: { owner: "acme", name: "node-client" }, number: 482 },
        head_sha: "x",
      });
      const stream = await openEvents();
      const events = await stream.take(9);
      stream.close();
      expect(events[0]).toMatchObject({ event: "analysis", summary: { state: { state: "planning" } } });
      expect(events[1]).toMatchObject({ event: "question", question: { step_id: "f1.5" } });
      expect(states(events).slice(1)).toEqual([
        ...[0, 1, 2, 3, 4].map((done) => ({
          state: "running",
          done,
          total: 5,
          idle: false,
          stage: {
            kind: "flow",
            flow: { index: done + 1, title: FLOW_TITLES[done] },
            started_at: expect.any(String),
          },
        })),
        { state: "running", done: 5, total: 5, idle: false },
        { state: "finished", total: 5 },
      ]);
    },
    10 * RUN_STEP_MS,
  );

  it("a pinned scenario state does not run; cancel and retry change it and reach the stream", async () => {
    setScenario("failed");
    const api = createApiClient(BASE);
    expect((await api.getAnalysis(ID)).state).toMatchObject({ state: "failed", flow: { id: "f3" } });
    const stream = await openEvents();
    const snapshot = await stream.take(1);
    expect((await api.retryAnalysis(ID)).state).toEqual({ state: "planning" });
    await api.cancelAnalysis(ID);
    const events = [...snapshot, ...(await stream.take(2))];
    stream.close();
    expect(states(events)).toEqual([
      { state: "failed", flow: { id: "f3", title: "Classify RPC errors" }, message: expect.any(String) },
      { state: "planning" },
      { state: "cancelled" },
    ]);
    expect(await api.retryAnalysis(ID).catch((e: unknown) => e)).toMatchObject({
      status: 409,
      code: "conflict",
    });
  });

  it("settings: default, set language, repository settings", async () => {
    const api = createApiClient(BASE);
    expect(await api.getSettings()).toMatchObject({
      language: "en",
      llm_status: { status: { state: "working" } },
    });
    await api.setLanguage({ language: "pl" });
    expect((await api.getSettings()).language).toBe("pl");
    expect((await api.getRepoSettings({ owner: "acme", name: "node-client" })).rules.length).toBeGreaterThan(
      0,
    );
    expect(
      await api.getRepoSettings({ owner: "acme", name: "other" }).catch((e: unknown) => e),
    ).toMatchObject({ status: 404, code: "not_found" });
  });
});
