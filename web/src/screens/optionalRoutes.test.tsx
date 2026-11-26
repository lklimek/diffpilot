import { QueryClient } from "@tanstack/react-query";
import { act, screen, waitFor } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { expect, it, vi } from "vitest";
import { renderRoute, startMockServer } from "../__tests__/screen";
import { routes } from "../api/generated/routes";

const server = startMockServer();

it.each([
  {
    route: "listMyPrs",
    page: "/prs",
    key: ["pr-history"],
    missing: { pages: [null] },
    fallback: () => screen.findByRole("list", { name: "Your pull requests" }),
  },
  {
    route: "getRankingUsage",
    page: "/settings/llm",
    key: ["settings", "ranking-usage"],
    missing: null,
    fallback: () => screen.findByText("Not available yet", { exact: true }),
  },
  {
    route: "getGithubWebhook",
    page: "/settings/github-sync",
    key: ["github-webhook"],
    missing: null,
    fallback: () => screen.findByText("Unavailable.", { exact: true }),
  },
  {
    route: "getRepositoryInstallations",
    page: "/settings/repositories",
    key: ["repository-installations"],
    missing: null,
    fallback: async () => (await screen.findAllByText("Installation not checked"))[0],
  },
] as const)(
  "reserved_route_501_treated_as_absent_and_not_polled: $route",
  async ({ route, page, key, missing, fallback }) => {
    const read = vi.fn(() =>
      HttpResponse.json({ code: "not_implemented", message: "Route unavailable." }, { status: 501 }),
    );
    server.use(http.get(routes[route].path, read));
    const client = new QueryClient({ defaultOptions: { queries: { retry: 1 } } });
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      renderRoute(page, client);
      await waitFor(() => {
        const data = client.getQueryData(key);
        if (missing === null) expect(data).toBeNull();
        else expect(data).toMatchObject(missing);
      });
      await act(() => vi.advanceTimersByTimeAsync(120_000));
      expect(read).toHaveBeenCalledOnce();
      expect(await fallback()).toBeVisible();
    } finally {
      vi.useRealTimers();
    }
  },
);
