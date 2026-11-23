import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { PluginNote } from "../../../api/generated/types";
import { clockText } from "../../format/time";
import { NOW } from "../test-fixtures";
import { PluginNotes } from "./PluginNotes";

const NOTE: PluginNote = {
  plugin: "pr-hygiene",
  source: "PR Hygiene",
  url: "https://pr-hygiene.example.test/api/v1/prs/acme/node-client/476",
  as_of: "2026-11-18T10:12:00Z",
  summary: "thepastaclaw requested changes on this head; dismiss the review or push a fix",
  details: [
    "thepastaclaw requested changes on this head; dismiss the review or push a fix",
    "Human approval or objection resolution is required",
  ],
  facts: [
    { label: "Stage", value: "self-review" },
    { label: "Whose move", value: "author" },
    { label: "Lateness", value: "late" },
  ],
};

describe("PluginNotes", () => {
  it("keeps the headline compact and attributes the evidence link", () => {
    render(<PluginNotes notes={[NOTE]} itemId="NC-476" now={NOW} />);
    const note = document.querySelector("[data-plugin-note='pr-hygiene']") as HTMLElement;
    expect(note).toHaveTextContent(
      "PR Hygiene: thepastaclaw requested changes on this head; dismiss the review or push a fix",
    );
    expect(screen.getByText(/Stage: self-review/)).not.toBeVisible();
    expect(within(note).getByRole("link", { name: "PR Hygiene data for NC-476 (new tab)" })).toHaveAttribute(
      "href",
      NOTE.url,
    );
    expect(screen.queryByRole("list")).toBeNull();
  });

  it.each(["focus", "hover"])(
    "shows every attributed point on %s and dismisses with Escape",
    async (method) => {
      render(<PluginNotes notes={[NOTE]} itemId="NC-476" now={NOW} />);
      const trigger = screen.getByLabelText("PR Hygiene statement for NC-476");
      if (method === "focus") fireEvent.focus(trigger);
      else await userEvent.hover(trigger);
      const tip = screen.getByRole("tooltip");
      expect(tip).toHaveTextContent(`PR Hygiene: ${NOTE.summary}`);
      expect(tip).toHaveTextContent("Human approval or objection resolution is required");
      for (const fact of NOTE.facts) expect(tip).toHaveTextContent(`${fact.label}: ${fact.value}`);
      expect(tip).toHaveTextContent(`as of ${clockText(NOTE.as_of, NOW)}`);
      expect(within(tip).queryByRole("link")).toBeNull();
      fireEvent.keyDown(trigger, { key: "Escape" });
      expect(screen.queryByRole("tooltip")).toBeNull();
    },
  );

  it("handles another plug-in and treats tooltip details and facts as untrusted text", () => {
    const { container } = render(
      <PluginNotes
        notes={[
          {
            ...NOTE,
            plugin: "thepastaclaw",
            source: "thepastaclaw",
            summary: "reviewing",
            details: ["<a href='https://evil.test'>click</a>"],
            facts: [{ label: "<b>Phase</b>", value: "<img src=x onerror=alert(1)>" }],
          },
        ]}
        itemId="NC-476"
        now={NOW}
      />,
    );
    fireEvent.focus(screen.getByLabelText("thepastaclaw statement for NC-476"));
    const tip = screen.getByRole("tooltip");
    expect(tip).toHaveTextContent("thepastaclaw: reviewing");
    expect(tip).toHaveTextContent("<a href='https://evil.test'>click</a>");
    expect(tip).toHaveTextContent("<b>Phase</b>: <img src=x onerror=alert(1)>");
    expect(tip.querySelector("a, b, img")).toBeNull();
    expect(container.querySelectorAll("a")).toHaveLength(1);
  });

  it("lists the further points only when full, without repeating the headline", () => {
    render(<PluginNotes notes={[NOTE]} itemId="NC-476" full now={NOW} />);
    const points = screen.getByRole("list", { name: "PR Hygiene: further points" });
    expect(
      within(points)
        .getAllByRole("listitem")
        .map((li) => li.textContent),
    ).toEqual(["Human approval or objection resolution is required"]);
  });

  it("renders external text as text, drops links that are not http(s) and shows nothing without notes", () => {
    const { container, unmount } = render(
      <PluginNotes
        notes={[{ ...NOTE, summary: "<b>bold</b>", url: "javascript:alert(1)", facts: [] }]}
        itemId="NC-476"
        now={NOW}
      />,
    );
    expect(screen.getAllByText(/<b>bold<\/b>/)[0]).toBeInTheDocument();
    expect(container.querySelector("b")).toBeNull();
    expect(screen.queryByRole("link")).toBeNull();
    unmount();
    const empty = render(<PluginNotes notes={undefined} itemId="NC-476" now={NOW} />);
    expect(empty.container).toBeEmptyDOMElement();
    empty.unmount();
    const none = render(<PluginNotes notes={[]} itemId="NC-476" now={NOW} />);
    expect(none.container).toBeEmptyDOMElement();
  });

  it("says so when the source states no next step", () => {
    render(<PluginNotes notes={[{ ...NOTE, summary: null }]} itemId="NC-476" now={NOW} />);
    expect(document.querySelector("[data-plugin-note]")).toHaveTextContent("PR Hygiene: no next step stated");
  });
});
