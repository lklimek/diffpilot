import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import type { WorkItem } from "../../../api/generated/types";
import { ITEMS, NOW, PROVISIONAL_ITEMS } from "../test-fixtures";
import { WorkList, type KindFilter } from "./WorkList";

const reviewHref = (item: WorkItem) => `/review?pr=${encodeURIComponent(item.url ?? "")}`;

function List(props: {
  items?: WorkItem[];
  initial?: KindFilter;
  onNavigate?: (href: string) => void;
  onHelp?: () => void;
}) {
  const [kind, setKind] = useState<KindFilter>(props.initial ?? "pr");
  return (
    <>
      <input aria-label="search" />
      <WorkList
        items={props.items ?? ITEMS}
        kind={kind}
        onKindChange={setKind}
        reviewHref={reviewHref}
        onNavigate={props.onNavigate}
        onHelp={props.onHelp}
        now={NOW}
      />
    </>
  );
}

const rowIds = () =>
  [...document.querySelectorAll("li[data-item]")].map((li) => li.getAttribute("data-item"));

describe("WorkList", () => {
  it("shows pull requests and TODOs on PRs, P0 alarms of every kind, issues only on Issues or Both", async () => {
    render(<List />);
    expect(screen.getByRole("button", { name: /PRs 8/ })).toHaveAttribute("aria-pressed", "true");
    expect(rowIds()).toContain("acme~platform~pr~1203");
    expect(rowIds()).toContain("memcan~3fa91c2e-0000-4000-8000-000000000000");
    expect(rowIds()).not.toContain("acme~platform~issue~1210");
    // The P0 issue stays visible as an alarm.
    expect(screen.getByRole("group", { name: "P0 alarm PLAT-1187" })).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /Issues 4/ }));
    expect(rowIds()).toContain("acme~platform~issue~1210");
    expect(rowIds()).not.toContain("acme~platform~pr~1203");

    await userEvent.click(screen.getByRole("button", { name: /Both 12/ }));
    expect(rowIds()).toHaveLength(13);
  });

  it("groups items by tier in the server's order, with new items under Not judged yet", () => {
    render(<List items={PROVISIONAL_ITEMS} initial="both" />);
    const headings = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    expect(headings).toEqual([
      expect.stringMatching(/^P1/),
      expect.stringMatching(/^P2/),
      expect.stringMatching(/^P3/),
      expect.stringMatching(/^Not judged yet/),
    ]);
    const notJudged = screen.getByRole("heading", { name: /Not judged yet/ })
      .nextElementSibling as HTMLElement;
    expect(within(notJudged).getByText("fix(ui): restore the focus ring on tabs")).toBeInTheDocument();
    expect(within(notJudged).getByText("Not judged yet; the LLM ranks it next.")).toBeInTheDocument();
  });

  it("marks provisional scores with ~ and every estimate in the breakdown", async () => {
    render(<List items={PROVISIONAL_ITEMS} />);
    const score = screen.getByRole("button", { name: /Score provisional 90, show breakdown/ });
    expect(score).toHaveTextContent("~90");
    await userEvent.click(score);
    const severity = document.querySelector(
      '[data-item="acme~wallet-ui~pr~320"] tr[data-part="defect_severity"]',
    )!;
    expect(severity).toHaveTextContent("estimate");
    expect(severity).toHaveTextContent("estimate until judged");
  });

  it("every LLM text carries the LLM tag", async () => {
    render(<List items={PROVISIONAL_ITEMS} initial="both" />);
    for (const button of screen.getAllByRole("button", { name: /^Details of/ }))
      await userEvent.click(button);
    const texts = [...document.querySelectorAll("[data-llm-text]")];
    expect(texts.length).toBeGreaterThan(20);
    for (const text of texts) expect(text.querySelector('[data-provenance="llm"]')).not.toBeNull();
    // LLM-judged parts name the LLM; computed parts say computed.
    for (const row of document.querySelectorAll("tr[data-part]")) {
      expect(row.querySelector("[data-source]")?.textContent).toMatch(
        /LLM|computed|estimate|not applicable|pending/,
      );
    }
  });

  it("j / k move the cursor, Enter expands, v opens the review, g the item, ? the help", () => {
    const onNavigate = vi.fn();
    const onHelp = vi.fn();
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    render(<List onNavigate={onNavigate} onHelp={onHelp} />);
    fireEvent.keyDown(document.body, { key: "j" });
    const first = document.querySelector("li[data-current]")!;
    expect(first.getAttribute("data-item")).toBe("acme~platform~pr~1203");
    expect(document.activeElement).toBe(
      within(first as HTMLElement).getByRole("button", { name: "Details of PLAT-1203" }),
    );

    fireEvent.keyDown(document.body, { key: "j" });
    fireEvent.keyDown(document.body, { key: "k" });
    expect(document.querySelector("li[data-current]")?.getAttribute("data-item")).toBe(
      "acme~platform~pr~1203",
    );

    fireEvent.keyDown(document.body, { key: "Enter" });
    expect(screen.getByRole("button", { name: "Details of PLAT-1203" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );

    fireEvent.keyDown(document.body, { key: "v" });
    expect(onNavigate).toHaveBeenCalledWith(
      `/review?pr=${encodeURIComponent("https://github.com/acme/platform/pull/1203")}`,
    );

    fireEvent.keyDown(document.body, { key: "g" });
    expect(open).toHaveBeenCalledWith(
      "https://github.com/acme/platform/pull/1203",
      "_blank",
      "noopener,noreferrer",
    );

    fireEvent.keyDown(document.body, { key: "?" });
    expect(onHelp).toHaveBeenCalledOnce();
    open.mockRestore();
  });

  it("ignores shortcuts while typing or with a modifier", () => {
    const onHelp = vi.fn();
    render(<List onHelp={onHelp} />);
    fireEvent.keyDown(screen.getByRole("textbox", { name: "search" }), { key: "j" });
    fireEvent.keyDown(document.body, { key: "j", ctrlKey: true });
    fireEvent.keyDown(screen.getByRole("textbox", { name: "search" }), { key: "?" });
    expect(document.querySelector("li[data-current]")).toBeNull();
    expect(onHelp).not.toHaveBeenCalled();
  });

  it("links a PR to its review: Review without an analysis, Open review when the head moved", () => {
    render(<List items={PROVISIONAL_ITEMS} />);
    const plain = document.querySelector('[data-item="acme~platform~pr~1203"] a[data-review-link]');
    expect(plain).toHaveTextContent("Review");
    const moved = document.querySelector('[data-item="acme~node-client~pr~482"] a[data-review-link]');
    expect(moved).toHaveTextContent("Open review");
    expect(moved).toHaveAttribute(
      "href",
      `/review?pr=${encodeURIComponent("https://github.com/acme/node-client/pull/482")}`,
    );
  });

  it("a plain click on a review link navigates in the app; a modified click keeps the browser's", () => {
    const onNavigate = vi.fn();
    render(<List onNavigate={onNavigate} />);
    const link = document.querySelector('[data-item="acme~platform~pr~1203"] a[data-review-link]')!;
    fireEvent.click(link, { ctrlKey: true });
    expect(onNavigate).not.toHaveBeenCalled();
    fireEvent.click(link);
    expect(onNavigate).toHaveBeenCalledOnce();
  });

  it("says when no item of the chosen kind needs the user", async () => {
    render(<List items={ITEMS.filter((item) => item.kind !== "issue")} />);
    await userEvent.click(screen.getByRole("button", { name: /Issues 0/ }));
    expect(screen.getByText("No issues need you.")).toBeInTheDocument();
  });

  it("tags and scores show their tooltip on keyboard focus", () => {
    render(<List />);
    const tag = document.querySelector('[data-item="acme~platform~pr~1203"] [data-fact-tag]') as HTMLElement;
    fireEvent.focus(tag);
    expect(screen.getByRole("tooltip")).toHaveTextContent(/requested/);
    fireEvent.blur(tag);
    const score = screen.getByRole("button", { name: /^Score 148/ });
    fireEvent.focus(score);
    expect(screen.getByRole("tooltip")).toHaveTextContent("Score 148 = sum of 12 parts");
  });
});
