import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { expect, it } from "vitest";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { unlinkedCodex } from "../../mocks/assessment";
import { setScenario } from "../../mocks/scenarios";

const server = startMockServer();

it("reads the last call even when job activity has no server endpoint", async () => {
  setScenario("llm-error");
  server.use(http.get("/api/v1/llm/activity", () => new HttpResponse(null, { status: 404 })));
  renderRoute("/");

  const trigger = await screen.findByRole("button", { name: "alice — LLM not working" });
  await userEvent.click(trigger);
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

  const trigger = await screen.findByRole("button", { name: /^alice — LLM paused until/ });
  await userEvent.click(trigger);
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
  await userEvent.click(await within(banner).findByRole("button", { name: "alice" }));
  await userEvent.click(within(banner).getByRole("button", { name: "Sign out" }));
  expect(await screen.findByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
  await waitFor(() => expect(router.state.location.pathname).toBe("/"));
  expect(requestBody).toEqual({ everywhere: false });
  expect(window.sessionStorage.getItem(draft)).toBeNull();
});
