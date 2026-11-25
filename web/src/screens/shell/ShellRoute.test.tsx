import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { expect, it, vi } from "vitest";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { unlinkedCodex } from "../../mocks/assessment";
import { idleActivity } from "../../mocks/llm-activity";
import { SETTINGS, setScenario } from "../../mocks/scenarios";

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

it("reads the last call even when job activity has no server endpoint", async () => {
  setScenario("llm-error");
  server.use(http.get("/api/v1/llm/activity", () => new HttpResponse(null, { status: 404 })));
  renderRoute("/");

  await userEvent.click(await accountButton());
  expect(await screen.findByRole("button", { name: "alice — LLM not working" })).toBeVisible();
  expect(await screen.findByText("Job activity unavailable.")).toBeVisible();
  expect(screen.getByText("The LLM provider's usage limit was reached.")).toBeVisible();
});

it("uses the current Codex account pause when the job route is absent", async () => {
  const pausedUntil = new Date(Date.now() + 60 * 60_000).toISOString();
  server.use(
    http.get("/api/v1/llm/activity", () => new HttpResponse(null, { status: 404 })),
    http.get("/api/v1/settings/codex", () =>
      HttpResponse.json({ ...unlinkedCodex, paused_until: pausedUntil }),
    ),
  );
  renderRoute("/review");

  await userEvent.click(await accountButton());
  expect(await screen.findByRole("button", { name: /^alice — LLM paused until/ })).toBeVisible();
  expect(screen.getByText(/^Paused at the usage limit until/)).toBeVisible();
});

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

it("shows the fallback attention indicator on mount without polling", async () => {
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
  try {
    const activity = vi.fn(() => new HttpResponse(null, { status: 404 }));
    const settings = vi.fn(() =>
      HttpResponse.json({
        ...SETTINGS,
        llm_status: {
          at: new Date().toISOString(),
          status: { state: "not_working", message: "LLM call failed." },
        },
      }),
    );
    const codex = vi.fn(() => HttpResponse.json(unlinkedCodex));
    server.use(
      http.get("/api/v1/llm/activity", activity),
      http.get("/api/v1/settings", settings),
      http.get("/api/v1/settings/codex", codex),
    );
    renderRoute("/review");
    const trigger = await screen.findByRole("button", { name: "alice — LLM not working" });
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    await act(() => vi.advanceTimersByTimeAsync(60_000));
    expect(activity).toHaveBeenCalledOnce();
    expect(settings).toHaveBeenCalledOnce();
    expect(codex).toHaveBeenCalledOnce();
    await userEvent.click(trigger);
    expect(await screen.findByText("Job activity unavailable.")).toBeVisible();
    expect(screen.getByText("LLM call failed.")).toBeVisible();
  } finally {
    vi.useRealTimers();
  }
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
