import { act, screen, waitFor, within } from "@testing-library/react";
import { http, HttpResponse } from "msw";
import { describe, expect, it, vi } from "vitest";
import { renderRoute, startMockServer } from "../__tests__/screen";
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
