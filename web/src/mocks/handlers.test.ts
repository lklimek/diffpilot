import { setupServer } from "msw/node";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { ApiError, createApiClient } from "../api/client";
import { routes } from "../api/generated/routes";
import type { AnalysisEvent, AnchoredAskRequest, AskQuestion, DecideRequest } from "../api/generated/types";
import { setFixtureState } from "./fixtures";
import { handlers } from "./handlers";
import { setMockLanguage } from "./handlers/get-settings";
import { RUN_STEP_MS, resetLive, setLiveState } from "./live-analysis";
import { PR, resetReviewLoop } from "./review-loop";
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
  resetReviewLoop();
  vi.useRealTimers();
});
afterAll(() => server.close());

const ID = "6d3a2f1e-0482-4c5e-9a7b-000000000482";

describe("msw handlers from fixtures/pr-482", () => {
  it("keeps an individual's mark when the primary is marked and undone", async () => {
    setScenario("comments-v14");
    const api = createApiClient(BASE);
    const items = await api.listCommentItems(PR);
    const request = (no: number, reason: string | null): DecideRequest => ({
      kind: "not_a_review_point",
      reason,
      item_revision: items.find((item) => item.no === no)!.revision,
      target: null,
      override: null,
      resolve_ticked: false,
      reply_text: null,
      comment: null,
    });
    const individual = await api.decideComment(PR, 30, request(30, null));
    await api.decideComment(PR, 14, request(14, "Fixture only."));
    const marked = await api.listCommentItems(PR);
    expect(marked.find((item) => item.no === 31)?.not_a_review_point?.follows).toBe(14);
    await api.undoCommentDecision(PR, 14, { item_revision: marked.find((item) => item.no === 14)!.revision });

    const log = await api.getCommentLog(PR);
    expect(log.marks?.filter((mark) => mark.item_no === 30)).toEqual([
      expect.objectContaining({ id: individual.id, undone_at: null }),
    ]);
    expect(log.marks?.find((mark) => mark.item_no === 31)).toMatchObject({
      follows: 14,
      undone_by: "alice",
    });
  });

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
    expect(lines.changed).toEqual([89, 91, 92, 93, 94, 96]);
    expect(lines.deleted.map((d) => d.after_line)).toEqual([88, 95]);
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
    const { sessionScope } = await api.getBrowserSession();
    await api.setLanguage({ language: "pl" }, sessionScope);
    expect((await api.getSettings()).language).toBe("pl");
    expect((await api.getRepoSettings({ owner: "acme", name: "node-client" })).rules.length).toBeGreaterThan(
      0,
    );
    expect(
      await api.getRepoSettings({ owner: "acme", name: "other" }).catch((e: unknown) => e),
    ).toMatchObject({ status: 404, code: "not_found" });
  });

  it("files (R12): every PR file with hunks; Viewed (R13) is set once GitHub confirmed and kept", async () => {
    const api = createApiClient(BASE);
    const files = await api.getFiles(ID);
    expect(files.viewed_status).toBe("checked");
    expect(files.files.map((f) => f.path)).toContain("src/rpc/client.rs");
    expect(files.files.find((f) => f.path === "src/rpc/client.rs")?.viewed).toBe("unviewed");
    await api.setFileViewed(ID, { path: "src/rpc/client.rs", viewed: true });
    expect((await api.getFiles(ID)).files.find((f) => f.path === "src/rpc/client.rs")?.viewed).toBe("viewed");
    expect(
      await api.setFileViewed(ID, { path: "src/nope.rs", viewed: true }).catch((e: unknown) => e),
    ).toMatchObject({ status: 400, code: "invalid_request" });
    setScenario("viewed-refused");
    expect(
      await api.setFileViewed(ID, { path: "src/lib.rs", viewed: true }).catch((e: unknown) => e),
    ).toMatchObject({ status: 422, code: "github_refused" });
    setScenario("viewed-busy");
    expect(
      await api.setFileViewed(ID, { path: "src/lib.rs", viewed: true }).catch((e: unknown) => e),
    ).toMatchObject({ status: 409, code: "conflict" });
    setScenario("viewed-signed-out");
    expect(
      await api.setFileViewed(ID, { path: "src/lib.rs", viewed: true }).catch((e: unknown) => e),
    ).toMatchObject({ status: 401, code: "unauthenticated" });
    setScenario("viewed-head-unread");
    expect(
      await api.setFileViewed(ID, { path: "src/lib.rs", viewed: true }).catch((e: unknown) => e),
    ).toMatchObject({
      status: 502,
      code: "github_unavailable",
      message:
        "Could not read the pull request from GitHub, so nothing was sent. Your mark is unchanged; try again.",
    });
    expect((await api.getFiles(ID)).files.find((f) => f.path === "src/lib.rs")?.viewed).toBe("unviewed");
  });

  it("files of a lazy tour: lines of a flow not analysed yet belong to that flow", async () => {
    setScenario("lazy-flows");
    const api = createApiClient(BASE);
    const config = (await api.getFiles(ID)).files.find((f) => f.path === "src/config.rs")!;
    expect(config.hunks[0]!.parts.map((p) => p.owner)).toContainEqual({ kind: "flow", flow_id: "f2" });
  });

  it("anchored Ask (R14): a lines question has no step", async () => {
    const api = createApiClient(BASE);
    const question = await api.askAnchored(ID, {
      text: "Why?",
      anchor: {
        kind: "lines",
        path: "src/rpc/metrics.rs",
        side: "head",
        range: { start_line: 41, end_line: 46 },
      },
    });
    expect(question).toMatchObject({ state: "pending", anchor: { kind: "lines" } });
    expect(question.step_id).toBeUndefined();
  });

  it("Ask chat: a follow-up keeps its thread (parent_id); flow and whole-PR questions are taken", async () => {
    const api = createApiClient(BASE);
    const ask = (body: AnchoredAskRequest) => api.askAnchored(ID, body);
    const root = (await ask({ text: "Why?", anchor: { kind: "pr" } })) as AskQuestion;
    expect(root).toMatchObject({ state: "pending", anchor: { kind: "pr" } });
    expect(root.parent_id).toBeUndefined();
    const follow = (await ask({
      text: "And then?",
      anchor: { kind: "pr" },
      parent_id: root.id,
    })) as AskQuestion;
    expect(follow.parent_id).toBe(root.id);
    const flow = (await ask({ text: "What?", anchor: { kind: "flow", flow_id: "f2" } })) as AskQuestion;
    expect(flow.anchor).toEqual({ kind: "flow", flow_id: "f2" });
    const listed = (await api.listQuestions(ID)) as AskQuestion[];
    expect(listed.find((q) => q.id === follow.id)?.parent_id).toBe(root.id);
    // Like the server: a follow-up of a follow-up joins the thread's first question; its anchor must match it.
    const deeper = (await ask({ text: "And?", anchor: { kind: "pr" }, parent_id: follow.id })) as AskQuestion;
    expect(deeper.parent_id).toBe(root.id);
    await expect(
      ask({ text: "?", anchor: { kind: "flow", flow_id: "f2" }, parent_id: root.id }),
    ).rejects.toMatchObject({
      status: 400,
    });
    await expect(ask({ text: "?", anchor: { kind: "pr" }, parent_id: "nope" })).rejects.toMatchObject({
      status: 404,
    });
    await expect(ask({ text: "?", anchor: { kind: "flow", flow_id: "f99" } })).rejects.toMatchObject({
      status: 404,
    });
    // Step lines must lie inside the step (f1.5 is head line 99), as on the server.
    await expect(
      ask({ text: "?", anchor: { kind: "step", step_id: "f1.5", lines: { start_line: 98, end_line: 99 } } }),
    ).rejects.toMatchObject({ status: 400 });
    expect(
      await ask({
        text: "?",
        anchor: { kind: "step", step_id: "f1.5", lines: { start_line: 99, end_line: 99 } },
      }),
    ).toMatchObject({ state: "pending" });
  });
});
