import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderRoute, startMockServer } from "../../__tests__/screen";
import { HELP_TOPICS, SETTINGS_HELP } from "../../components/settings/settingsHelp";

startMockServer();

describe("Settings help (owner request B3)", () => {
  it("every option on the page has a ? help; the page links the full guide", async () => {
    renderRoute("/settings");
    await screen.findByRole("region", { name: "Dashboard" });
    await waitFor(() => expect(screen.queryByText(/^Loading/)).toBeNull());
    for (const topic of HELP_TOPICS.filter((t) => SETTINGS_HELP[t].onPage !== false)) {
      const name = `Help: ${SETTINGS_HELP[topic].title}`;
      expect(screen.getAllByRole("link", { name })[0], name).toHaveAttribute(
        "href",
        `/settings/help#${topic}`,
      );
    }
    expect(screen.getByRole("link", { name: "Settings help" })).toHaveAttribute("href", "/settings/help");
  });

  it("the search fallback is named for when it is used, with a visible hint", async () => {
    renderRoute("/settings");
    const group = await screen.findByRole("group", { name: "When GitHub search refuses an account" });
    expect(group).toHaveTextContent(
      "Optional. Organisations or users whose open pull requests and issues are listed directly when GitHub search will not answer for one of your accounts. Example: dashpay",
    );
  });

  it("/settings/help shows the whole guide inside the shell, offline (no request needed)", async () => {
    renderRoute("/settings/help");
    expect(await screen.findByRole("heading", { level: 1, name: "Settings guide" })).toBeVisible();
    for (const topic of HELP_TOPICS)
      expect(screen.getByRole("heading", { level: 3, name: SETTINGS_HELP[topic].title })).toBeVisible();
    const nav = screen.getByRole("navigation", { name: "Main" });
    expect(within(nav).getByRole("link", { name: /^Settings/ })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("link", { name: "Back to Settings" })).toHaveAttribute("href", "/settings");
  });
});
