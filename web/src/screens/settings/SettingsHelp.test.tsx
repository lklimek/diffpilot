import { screen, within } from "@testing-library/react";
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
    ).toHaveAttribute("href", "/settings/help#enumerate_owners");
    expect(scope).toHaveTextContent(/listed directly when GitHub search will not answer/);
    expect(screen.getByRole("link", { name: "Settings help" })).toHaveAttribute("href", "/settings/help");
  });

  it("/settings/help shows the whole guide inside the shell, offline (no request needed)", async () => {
    renderRoute("/settings/help");
    expect(await screen.findByRole("heading", { level: 1, name: "Settings guide" })).toBeVisible();
    for (const topic of HELP_TOPICS)
      expect(screen.getByRole("heading", { level: 3, name: SETTINGS_HELP[topic].title })).toBeVisible();
    const nav = screen.getByRole("navigation", { name: "Main" });
    expect(within(nav).getByRole("link", { name: /^Settings/ })).not.toHaveAttribute("aria-current");
    expect(screen.getByRole("link", { name: "Back to Settings" })).toHaveAttribute("href", "/settings");
  });
});
