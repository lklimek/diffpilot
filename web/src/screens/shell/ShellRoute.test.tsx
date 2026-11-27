import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { expect, it, vi } from "vitest";
import { FakeEventSource } from "../../__tests__/fake-event-source";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { unlinkedCodex } from "../../mocks/assessment";
import dashboardItems from "../../../../fixtures/dashboard/api/items.json";
import type { ItemPage } from "../../api/generated/types";
import { failedActivity, idleActivity, pausedActivity } from "../../mocks/llm-activity";
import { SETTINGS } from "../../mocks/scenarios";

const server = startMockServer();
const renderShell = vi.hoisted(() => vi.fn());

vi.mock("./ShellLayout", async (importOriginal) => {
  const original = await importOriginal<typeof import("./ShellLayout")>();
  return {
    ShellLayout: (props: Parameters<typeof original.ShellLayout>[0]) => {
      renderShell();
      return <original.ShellLayout {...props} />;
    },
  };
});

const accountButton = () => screen.findByRole("button", { name: /^alice/ });

it.each(["/", "/review"])("signs out from %s using the existing session route", async (path) => {
  let requestBody: unknown;
  server.use(
    http.post("/auth/logout", async ({ request }) => {
      requestBody = await request.json();
      return HttpResponse.json(
        { code: "unauthenticated", message: "Session already ended." },
        { status: 401 },
      );
    }),
  );
  server.use(
    http.get("/api/v1/me", () =>
      HttpResponse.json(
        requestBody
          ? { user: null, csrf_token: null }
          : {
              user: { login: "alice" },
              csrf_token: "fake-csrf",
            },
      ),
    ),
  );
  const draft = "diffpilot.comment-drafts.alice.fixture";
  window.sessionStorage.setItem(draft, '{"comment":"draft"}');
  const router = renderRoute(path);
  const banner = screen.getByRole("banner");
  await userEvent.click(
    await within(banner).findByRole("button", { name: "alice: account and LLM activity" }),
  );
  await userEvent.click(within(banner).getByRole("button", { name: "Sign out" }));
  expect(await screen.findByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
  await waitFor(() => expect(router.state.location.pathname).toBe("/"));
  expect(requestBody).toEqual({ everywhere: false });
  expect(window.sessionStorage.getItem(draft)).toBeNull();
});

it("takes the activity route's last call and pause as authoritative without fetching Settings or Codex", async () => {
  const settings = vi.fn(() => HttpResponse.json(SETTINGS));
  const codex = vi.fn(() => HttpResponse.json(unlinkedCodex));
  server.use(
    http.get("/api/v1/llm/activity", () => HttpResponse.json(idleActivity)),
    http.get("/api/v1/settings", settings),
    http.get("/api/v1/settings/codex", codex),
  );
  renderRoute("/review");
  await userEvent.click(await accountButton());
  expect(await screen.findByText("Idle")).toBeVisible();
  expect(screen.getByText("Last LLM call outcome unknown.")).toBeVisible();
  expect(settings).not.toHaveBeenCalled();
  expect(codex).not.toHaveBeenCalled();
});

it("keeps a reserved job activity route absent without querying legacy sources", async () => {
  const activity = vi.fn(() => new HttpResponse(null, { status: 501 }));
  const settings = vi.fn(() => HttpResponse.json(SETTINGS));
  const codex = vi.fn(() => HttpResponse.json(unlinkedCodex));
  server.use(
    http.get("/api/v1/llm/activity", activity),
    http.get("/api/v1/settings", settings),
    http.get("/api/v1/settings/codex", codex),
  );
  renderRoute("/review");
  await userEvent.click(await accountButton());
  expect(await screen.findByText("Job activity unavailable.")).toBeVisible();
  expect(activity).toHaveBeenCalledOnce();
  expect(settings).not.toHaveBeenCalled();
  expect(codex).not.toHaveBeenCalled();
});

it("updates the activity in the menu without rendering the shell layout again", async () => {
  let answer!: () => void;
  const ready = new Promise<void>((resolve) => {
    answer = resolve;
  });
  const pausedUntil = new Date(Date.now() + 60 * 60_000).toISOString();
  server.use(
    http.get("/api/v1/llm/activity", async () => {
      await ready;
      return HttpResponse.json({ ...idleActivity, paused_until: pausedUntil });
    }),
  );
  renderRoute("/review");
  await accountButton();
  const renders = renderShell.mock.calls.length;

  answer();
  expect(await screen.findByRole("button", { name: /^alice — LLM paused until/ })).toBeVisible();
  expect(renderShell).toHaveBeenCalledTimes(renders);
});

it("updates import counts in an idle My work tab without a new revision", async () => {
  let imported = false;
  const items = dashboardItems as unknown as ItemPage;
  server.use(
    http.get("/api/v1/dashboard/items", ({ request }) => {
      const etag = imported ? '"imports"' : '"initial"';
      if (request.headers.get("If-None-Match") === etag) return new HttpResponse(null, { status: 304 });
      return HttpResponse.json(
        {
          ...items,
          items: items.items.map((item) => ({
            ...item,
            imported_unseen: imported && item.number === 482 && item.kind === "pr" ? 2 : 0,
          })),
        },
        { headers: { ETag: etag } },
      );
    }),
  );
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
  try {
    renderRoute("/");
    await screen.findByRole("link", { name: /acme.*node-client.*482/i });
    imported = true;
    await act(() => vi.advanceTimersByTimeAsync(61_000));
    expect(await screen.findByRole("link", { name: "2 new GitHub decisions" })).toHaveAttribute(
      "href",
      "/pr/acme/node-client/482/comments?imported_unseen=true",
    );
  } finally {
    vi.useRealTimers();
  }
});

it("uses LLM activity events without notification opt-in and refetches only on reconnect", async () => {
  FakeEventSource.reset();
  vi.stubGlobal("EventSource", FakeEventSource);
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
  let answer = idleActivity;
  const read = vi.fn(() => HttpResponse.json(answer));
  server.use(http.get("/api/v1/llm/activity", read));
  try {
    renderRoute("/review");
    await userEvent.click(await accountButton());
    expect(await screen.findByText("Idle")).toBeVisible();
    const source = FakeEventSource.instances.find(({ url }) => url === "/api/v1/events")!;
    expect(source).toBeDefined();
    act(() => source.open());
    act(() =>
      source.emit({
        event: "llm_activity",
        data: JSON.stringify({ event: "llm_activity", activity: failedActivity }),
      }),
    );
    expect(await screen.findByText("Codex sign-in expired.")).toBeVisible();
    await act(() => vi.advanceTimersByTimeAsync(61_000));
    expect(read).toHaveBeenCalledOnce();
    answer = pausedActivity;
    act(() => source.drop());
    act(() => source.open());
    expect(await screen.findByText(/^Paused at the usage limit until/)).toBeVisible();
    expect(read).toHaveBeenCalledTimes(2);
  } finally {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  }
});
