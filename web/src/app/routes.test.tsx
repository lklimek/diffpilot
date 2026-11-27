import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { http, HttpResponse } from "msw";
import { describe, expect, it, vi } from "vitest";
import type { AnalysisSummary } from "../api/generated/types";
import { ITEMS } from "../components/dashboard/test-fixtures";
import { renderRoute, startMockServer } from "../__tests__/screen";
import { seedFor } from "../mocks/fixtures";
import { setScenario } from "../mocks/scenarios";

const server = startMockServer();
const PR_URL = "https://github.com/acme/node-client/pull/482";

const nav = () => screen.findByRole("navigation", { name: "Main" });
const navLink = async (name: string) =>
  within(await nav()).getByRole("link", { name: new RegExp(`^${name}`) });

describe("routes after the switch (plan §F.1)", () => {
  it("/ is My work inside the shell; the shell links My work, Pull requests and Settings", async () => {
    const router = renderRoute("/");
    expect(await navLink("My work")).toHaveAttribute("aria-current", "page");
    expect(await navLink("My work")).toHaveAttribute("href", "/");
    expect(await navLink("Pull requests")).toHaveAttribute("href", "/prs");
    expect(await navLink("Settings")).toHaveAttribute("href", "/settings");
    expect(screen.queryByLabelText("GitHub pull request URL")).toBeNull();
    expect(router.state.location.pathname).toBe("/");
  });

  it("/ signed out shows the sign-in panel", async () => {
    setScenario("signed-out");
    renderRoute("/");
    expect(await screen.findByRole("link", { name: "Sign in with GitHub" })).toBeVisible();
  });

  it("/review redirects to Pull requests inside the same shell", async () => {
    renderRoute("/review");
    expect(await screen.findByLabelText("GitHub pull request URL")).toBeVisible();
    expect(await navLink("Pull requests")).toHaveAttribute("aria-current", "page");
  });

  it.each([
    ["/review/", "Pull requests"],
    ["/settings/", "Account"],
  ])("%s (trailing slash) marks %s as the current page", async (entry, name) => {
    renderRoute(entry);
    expect(await navLink(name)).toHaveAttribute("aria-current", "page");
    expect(await navLink("My work")).not.toHaveAttribute("aria-current");
  });

  it("/settings is inside the shell", async () => {
    renderRoute("/settings");
    expect(await navLink("Account")).toHaveAttribute("aria-current", "page");
    expect(await navLink("Settings")).not.toHaveAttribute("aria-current");
  });

  it.each([
    [`/?pr=${encodeURIComponent(PR_URL)}`, "/pr/acme/node-client/482", ""],
    ["/?login_error=oauth_failed", "/prs", "?login_error=oauth_failed"],
    ["/?login_error=oauth_failed&pr=x", "/prs", "?login_error=oauth_failed&pr=x"],
  ])("%s keeps the relevant query after redirecting", async (entry, path, search) => {
    const router = renderRoute(entry);
    await waitFor(() => expect(router.state.location.pathname).toBe(path));
    expect(router.state.location.search).toBe(search);
    expect(router.state.historyAction).toBe("REPLACE");
  });

  it("the OAuth error still reaches its explanation through the redirect", async () => {
    setScenario("signed-out");
    renderRoute("/?login_error=oauth_failed");
    expect(await screen.findByRole("alert")).toHaveTextContent("Sign-in with GitHub did not complete.");
  });

  it("/review?pr= checks the PR on arrival and starts nothing", async () => {
    const created = vi.fn();
    server.use(
      http.post("/api/v1/analyses", () => {
        created();
        return HttpResponse.json({}, { status: 500 });
      }),
    );
    const router = renderRoute(`/review?pr=${encodeURIComponent(PR_URL)}`);
    expect(await screen.findByRole("region", { name: "Review the code" })).toBeVisible();
    expect(await screen.findByRole("button", { name: /Start analysis/ })).toBeVisible();
    expect(created).not.toHaveBeenCalled();
    expect(router.state.location.pathname).toBe("/pr/acme/node-client/482");
  });

  it("moving from another shell page to My work keeps the shell and shows My work", async () => {
    const router = renderRoute("/review");
    await screen.findByLabelText("GitHub pull request URL");
    await act(() => router.navigate("/"));
    expect(await navLink("My work")).toHaveAttribute("aria-current", "page");
    expect(screen.queryByLabelText("GitHub pull request URL")).toBeNull();
  });

  it("an unknown path keeps the not-found page, which leads to My work", async () => {
    renderRoute("/nowhere");
    expect(await screen.findByRole("link", { name: "My work" })).toHaveAttribute("href", "/");
  });
});

describe("PR-scoped analysis links", () => {
  const id = "6d3a2f1e-0482-4c5e-9a7b-000000000482";
  const base = `/pr/acme/node-client/482/analyses/${id}`;

  it.each(["", "/files", "/deps"])(
    "redirects an old bookmark for %s with its query and fragment",
    async (suffix) => {
      const router = renderRoute(`/analyses/${id}${suffix}?scenario=example#pr_context`);
      await waitFor(() =>
        expect(router.state.location).toMatchObject({
          pathname: base + suffix,
          search: "?scenario=example",
          hash: "#pr_context",
        }),
      );
      expect(router.state.historyAction).toBe("REPLACE");
    },
  );

  it.each(["", "/files", "/deps", "/progress"])(
    "denies a mismatched PR for %s before loading the view",
    async (suffix) => {
      const requested = "3ca11dd2-9b41-4c1c-9d3d-bf52558140e2";
      const loaded = vi.fn();
      const summary = seedFor("default", "analysis") as AnalysisSummary;
      server.use(
        http.get(`/api/v1/analyses/${requested}`, () => HttpResponse.json({ ...summary, id: requested })),
        http.get(`/api/v1/analyses/${requested}/tour`, () => {
          loaded();
          return new HttpResponse(null, { status: 500 });
        }),
      );
      renderRoute(`/pr/acme/node-client/999/analyses/${requested}${suffix}`);
      expect(await screen.findByRole("alert")).toHaveTextContent(
        "This analysis does not belong to this pull request.",
      );
      expect(loaded).not.toHaveBeenCalled();
    },
  );

  it("preserves the API refusal without redirecting to any PR", async () => {
    server.use(
      http.get("/api/v1/analyses/:id", () =>
        HttpResponse.json({ code: "forbidden", message: "Analysis access denied." }, { status: 403 }),
      ),
    );
    const router = renderRoute(`/analyses/${id}/files`);
    expect(await screen.findByRole("alert")).toHaveTextContent("Analysis access denied.");
    expect(router.state.location.pathname).toBe(`/analyses/${id}/files`);
  });

  it("shows the app navigation on the PR page and restores it after collapsing", async () => {
    const user = userEvent.setup();
    renderRoute("/pr/acme/node-client/482");
    expect(await navLink("My work")).toHaveAttribute("href", "/");
    expect(await navLink("Pull requests")).toHaveAttribute("href", "/prs");
    expect(await navLink("Settings")).toHaveAttribute("href", "/settings");
    await user.click(within(await nav()).getByRole("button", { name: "Collapse navigation" }));
    const expand = within(await nav()).getByRole("button", { name: "Expand navigation" });
    expect(expand).toHaveAttribute("aria-expanded", "false");
    await user.click(expand);
    expect(within(await nav()).getByRole("button", { name: "Collapse navigation" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
  });
});

it("shows the authorized matching My work ranking and its facts on the PR page", async () => {
  renderRoute("/pr/acme/node-client/482");
  const work = within(await screen.findByRole("region", { name: "My work" }));
  expect(await work.findByRole("button", { name: "Details of NC-482" })).toHaveAttribute(
    "aria-expanded",
    "true",
  );
  expect(work.getByRole("button", { name: /Score .*breakdown/ })).toBeVisible();
  expect(work.queryByRole("button", { name: "Details of PLAT-1203" })).toBeNull();
});

it("shows the My work alarm reason on the PR page", async () => {
  const item = ITEMS.find((entry) => entry.id === "NC-482")!;
  server.use(
    http.get("/api/v1/dashboard/items", () =>
      HttpResponse.json({
        items: [
          {
            ...item,
            score: { ...item.score, tier: "p0" },
            llm: { ...item.llm, alarm: "The maintainer is blocked on a review." },
          },
        ],
        next_cursor: null,
      }),
    ),
  );
  renderRoute("/pr/acme/node-client/482");
  const work = within(await screen.findByRole("region", { name: "My work" }));
  const alarm = within(await work.findByRole("group", { name: "P0 alarm NC-482" }));
  expect(alarm.getByText("The maintainer is blocked on a review.")).toBeVisible();
});

it("keeps My work PR details behind the PR page's own access check", async () => {
  server.use(
    http.get("/api/v1/prs/acme/node-client/482", () =>
      HttpResponse.json({ code: "not_found", message: "Pull request unavailable." }, { status: 404 }),
    ),
  );
  renderRoute("/pr/acme/node-client/482");
  expect(await screen.findByRole("alert")).toHaveTextContent("Pull request unavailable.");
  expect(screen.queryByRole("region", { name: "My work" })).toBeNull();
});
