import { render, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { PluginStatus } from "../../../api/generated/types";
import { PluginStatusNote } from "./PluginStatusNote";

const PLUGIN: PluginStatus = {
  plugin: "pr-hygiene",
  source: "PR Hygiene",
  url: "https://pr-hygiene.example.test/",
  state: "waiting",
  as_of: null,
};

describe("PluginStatusNote", () => {
  it("names a plug-in not read yet or stale, attributed and linked to its home page", () => {
    const { container, unmount } = render(
      <PluginStatusNote plugins={[PLUGIN]} what="Ready to merge is shown" />,
    );
    const line = container.querySelector("[data-plugin-status='waiting']") as HTMLElement;
    expect(line).toHaveTextContent(
      "PR Hygiene: not read yet; Ready to merge is shown without its statements.",
    );
    expect(within(line).getByRole("link", { name: "PR Hygiene home page (new tab)" })).toHaveAttribute(
      "href",
      PLUGIN.url,
    );
    unmount();
    const stale = render(
      <PluginStatusNote plugins={[{ ...PLUGIN, state: "stale" }]} what="Ready to merge is shown" />,
    );
    expect(stale.container).toHaveTextContent("PR Hygiene: unavailable or out of date;");
  });

  it("shows nothing for a current plug-in or without plug-ins", () => {
    for (const plugins of [undefined, [], [{ ...PLUGIN, state: "current" as const }]]) {
      const { container, unmount } = render(
        <PluginStatusNote plugins={plugins} what="Ready to merge is shown" />,
      );
      expect(container).toBeEmptyDOMElement();
      unmount();
    }
  });
});
