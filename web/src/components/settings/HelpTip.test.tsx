import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { HelpTip } from "./HelpTip";
import { SettingsHelpGuide } from "./SettingsHelpGuide";
import { HELP_TOPICS, SETTINGS_HELP } from "./settingsHelp";

it("the ? tip names its option, opens the guide at it in a new tab and describes what, when and an example", () => {
  render(<HelpTip topic="enumerate_owners" />);
  const link = screen.getByRole("link", { name: "Help: When GitHub search refuses an account" });
  expect(link).toHaveAttribute("href", "/settings/help#enumerate_owners");
  expect(link).toHaveAttribute("target", "_blank");
  expect(link).toHaveTextContent("?");
  expect(link).toHaveAccessibleDescription(/What: Optional\. Organisations or users/);
  expect(link).toHaveAccessibleDescription(/When used: /);
  expect(link).toHaveAccessibleDescription(/Example: dashpay\./);
});

it("the guide lists every option by section, each with an anchor, its lines and a link to stale_prs_are_bad", () => {
  render(<SettingsHelpGuide />);
  for (const topic of HELP_TOPICS) {
    const heading = screen.getByRole("heading", { level: 3, name: SETTINGS_HELP[topic].title });
    expect(heading.closest("section")).toHaveAttribute("id", topic);
  }
  expect(screen.getByRole("heading", { level: 2, name: "Dashboard" })).toBeVisible();
  expect(
    screen.getAllByRole("link", { name: "https://github.com/dashpay/stale_prs_are_bad" })[0],
  ).toHaveAttribute("href", "https://github.com/dashpay/stale_prs_are_bad");
});
