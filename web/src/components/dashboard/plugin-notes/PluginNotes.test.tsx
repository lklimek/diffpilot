import { render, screen, within } from "@testing-library/react";
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
  it("attributes the headline to its source, with the facts, the data time and the evidence link", () => {
    render(<PluginNotes notes={[NOTE]} itemId="NC-476" now={NOW} />);
    const note = document.querySelector("[data-plugin-note='pr-hygiene']") as HTMLElement;
    expect(note).toHaveTextContent(
      "PR Hygiene: thepastaclaw requested changes on this head; dismiss the review or push a fix",
    );
    expect(note).toHaveTextContent(
      `Stage: self-review · Whose move: author · Lateness: late · as of ${clockText(NOTE.as_of, NOW)}`,
    );
    expect(within(note).getByRole("link", { name: "PR Hygiene data for NC-476 (new tab)" })).toHaveAttribute(
      "href",
      NOTE.url,
    );
    expect(screen.queryByRole("list")).toBeNull();
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
    expect(screen.getByText(/<b>bold<\/b>/)).toBeInTheDocument();
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
