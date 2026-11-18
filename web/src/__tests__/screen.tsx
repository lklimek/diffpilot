import { RouterProvider } from "react-router/dom";
// Renders the app's routes in memory against the msw handlers of src/mocks (screen tests).
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import { setupServer } from "msw/node";
import { createMemoryRouter, type InitialEntry } from "react-router";
import { afterAll, afterEach, beforeAll } from "vitest";
import { reviewStates } from "../api/review-state";
import { appRoutes } from "../app/routes";
import { setFixtureState } from "../mocks/fixtures";
import { handlers } from "../mocks/handlers";
import { setMockLanguage } from "../mocks/handlers/get-settings";
import { resetLazyFlows } from "../mocks/lazy-flows";
import { resetLive } from "../mocks/live-analysis";
import { resetStore } from "../mocks/review-store";
import { setScenario } from "../mocks/scenarios";

/** Starts the msw server for the file's tests and resets every mock state after each test. */
export function startMockServer() {
  const server = setupServer(...handlers);
  beforeAll(() => server.listen({ onUnhandledFrame: "error" }));
  afterEach(() => {
    server.resetHandlers();
    setFixtureState("default");
    setScenario(null);
    setMockLanguage(null);
    resetLive();
    resetLazyFlows();
    resetStore();
    reviewStates.reset();
  });
  afterAll(() => server.close());
  return server;
}

/** Renders the app at `entry`; returns the router (its `state.location` is where the app went). */
export function renderRoute(entry: InitialEntry) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const router = createMemoryRouter(appRoutes, { initialEntries: [entry] });
  render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return router;
}
