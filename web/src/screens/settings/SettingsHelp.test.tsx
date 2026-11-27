import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { HELP_TOPICS, SETTINGS_HELP } from "../../components/settings/settingsHelp";

startMockServer();

describe("Settings help (owner request B3)", () => {
  it("the scope links field help and the full guide", async () => {
    renderRoute("/settings/github-sync");
    const scope = await screen.findByRole("region", { name: "Scope" });
    expect(
      within(scope).getByRole("link", { name: `Help: ${SETTINGS_HELP.enumerate_owners.title}` }),
    ).toHaveAttribute("href", "/settings/help?from=%2Fsettings%2Fgithub-sync#enumerate_owners");
    expect(scope).toHaveTextContent(/listed directly when GitHub search will not answer/);
    expect(screen.getByRole("link", { name: "Settings help" })).toHaveAttribute(
      "href",
      "/settings/help?from=%2Fsettings%2Fgithub-sync",
    );
  });

  it("/settings/help shows the whole guide inside the shell, offline (no request needed)", async () => {
    renderRoute("/settings/help");
    expect(await screen.findByRole("heading", { level: 1, name: "Settings guide" })).toBeVisible();
    expect(screen.getAllByRole("heading", { level: 2 }).map((heading) => heading.textContent)).toEqual([
      "Account",
      "LLM & usage",
      "GitHub sync",
      "Repositories",
      "Review loop",
      "MemCan",
    ]);
    for (const topic of HELP_TOPICS)
      expect(screen.getByRole("heading", { level: 3, name: SETTINGS_HELP[topic].title })).toBeVisible();
    const nav = screen.getByRole("navigation", { name: "Main" });
    expect(within(nav).getByRole("link", { name: /^Settings/ })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("link", { name: "Back to Settings" })).toHaveAttribute(
      "href",
      "/settings/account",
    );
  });
});

it("returns from the guide to the originating settings page and its repository", async () => {
  const router = renderRoute("/settings/repositories?repo=acme%2Fnode-client");
  await userEvent.click(await screen.findByRole("link", { name: "Settings help" }));
  await userEvent.click(await screen.findByRole("link", { name: "Back to Settings" }));
  expect(router.state.location.pathname).toBe("/settings/repositories");
  expect(router.state.location.search).toBe("?repo=acme%2Fnode-client");
});

it("returns to My work when its help link opened the guide", async () => {
  renderRoute("/settings/help?from=%2F%3Ffilter%3Dtest");
  expect(await screen.findByRole("link", { name: "Back to My work" })).toHaveAttribute(
    "href",
    "/?filter=test",
  );
});

it("falls back to Account when a return URL is outside the app", async () => {
  renderRoute("/settings/help?from=%2F%2Fexample.test");
  expect(await screen.findByRole("link", { name: "Back to Settings" })).toHaveAttribute(
    "href",
    "/settings/account",
  );
});
