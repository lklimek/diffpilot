import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ageBuckets, QueueAge } from "../queue-age/QueueAge";
import { ScanNotes } from "../scan-notes/ScanNotes";
import { NOW, SUMMARY } from "../test-fixtures";
import { ThroughputMinis } from "../throughput-minis/ThroughputMinis";
import { WaitingOnOthers } from "../waiting-on-others/WaitingOnOthers";
import { Yesterday } from "../yesterday/Yesterday";
import { SIDE_PANEL_STORAGE_KEY, SidePanel } from "./SidePanel";

function viewport(width: number) {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: width >= Number(/min-width: (\d+)px/.exec(query)?.[1] ?? 0),
    media: query,
  }));
}

const panel = () =>
  render(
    <SidePanel main={<p>main</p>} railText="More · waiting on others 3">
      <p>sections</p>
    </SidePanel>,
  );

describe("SidePanel", () => {
  beforeEach(() => window.localStorage.clear());
  afterEach(() => vi.unstubAllGlobals());

  it("is collapsed below 1600 px and opens from its rail", async () => {
    viewport(1440);
    panel();
    expect(screen.queryByText("sections")).toBeNull();
    const rail = screen.getByRole("button", { name: /More · waiting on others 3/ });
    expect(rail).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(rail);
    expect(screen.getByText("sections")).toBeInTheDocument();
    expect(JSON.parse(window.localStorage.getItem(SIDE_PANEL_STORAGE_KEY)!)).toMatchObject({ open: true });
  });

  it("is open from 1600 px and remembers a collapse", async () => {
    viewport(3440);
    const { unmount } = panel();
    expect(screen.getByText("sections")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Collapse the side panel" }));
    unmount();
    panel();
    expect(screen.queryByText("sections")).toBeNull();
  });

  it("resizes with the arrow keys on its splitter, remembers the width and resets on double-click", () => {
    viewport(3440);
    // Before any resize the value is the default width, clamp(340px, 22vw, 560px).
    vi.stubGlobal("innerWidth", 2000);
    panel();
    const splitter = screen.getByRole("separator", { name: "Resize the side panel" });
    expect(splitter).toHaveAttribute("aria-valuenow", "440");
    fireEvent.keyDown(splitter, { key: "ArrowLeft" });
    expect(splitter).toHaveAttribute("aria-valuenow", "464");
    fireEvent.keyDown(splitter, { key: "ArrowRight" });
    fireEvent.keyDown(splitter, { key: "ArrowRight" });
    expect(splitter).toHaveAttribute("aria-valuenow", "416");
    expect(JSON.parse(window.localStorage.getItem(SIDE_PANEL_STORAGE_KEY)!)).toMatchObject({ width: 416 });
    fireEvent.doubleClick(splitter);
    expect(splitter).toHaveAttribute("aria-valuenow", "440");
  });
});

describe("side panel sections", () => {
  it("lists PRs waiting on others, longest wait first, with the days and scan-fact tags", () => {
    render(<WaitingOnOthers items={SUMMARY.snapshot!.awaiting} now={NOW} />);
    expect(screen.getByText("Waiting on others").closest("summary")).toHaveTextContent("3 PRs · oldest 11 d");
    const items = screen.getAllByRole("listitem");
    expect(items[0]).toHaveTextContent("PLAT-1176");
    expect(items[0]).toHaveTextContent("waits on @jun-park, team:core for 11 d");
    // Waiting items are never ranked: no score column, no focusable placeholder.
    expect(items[2]).not.toHaveTextContent("—");
    expect(screen.queryByText(/Not ranked/)).toBeNull();
    const first = SUMMARY.snapshot!.awaiting.find((item) => item.id === "PLAT-1176")!;
    const link = within(items[0]!).getByRole("link", { name: "Open PLAT-1176 on GitHub" });
    expect(link).toHaveAttribute("href", first.url);
    expect(link).toHaveAttribute("target", "_blank");
    expect(link).toHaveAccessibleDescription("Open PLAT-1176 on GitHub");
  });

  it("shows yesterday's PRs and reviews only", async () => {
    render(<Yesterday label="Tue 17 Nov" prs={{ closed: 6, merged: 5, opened: 2 }} reviews={5} />);
    expect(screen.getByText("Yesterday").closest("summary")).toHaveTextContent("PRs net +4 · 5 reviews");
    await userEvent.click(screen.getByText("Yesterday"));
    expect(screen.getAllByRole("row").map((row) => row.textContent)).toEqual([
      "Tue 17 NovClosedOpenedNetNote",
      "PRs62+45 merged",
    ]);
    // Reviews have no closed or opened count: their own line, not a row under "Closed".
    expect(screen.getByText(/^Reviews given:/)).toHaveTextContent("Reviews given: 5");
  });

  it("colours yesterday's net by its sign in the summary and the table; counts and zero stay neutral", async () => {
    const tone = (el: Element | null) =>
      el?.classList.contains("text-ok") ? "ok" : el?.classList.contains("text-high") ? "high" : "neutral";
    const { unmount } = render(
      <Yesterday label="Tue 17 Nov" prs={{ closed: 1, merged: 1, opened: 4 }} reviews={2} />,
    );
    const summary = screen.getByText("Yesterday").closest("summary")!;
    expect(tone(within(summary).getByText("−3"))).toBe("high");
    await userEvent.click(screen.getByText("Yesterday"));
    const cells = within(screen.getAllByRole("row")[1]!).getAllByRole("cell");
    expect(tone(within(cells[2]!).getByText("−3"))).toBe("high");
    expect([tone(cells[0]!), tone(cells[1]!)]).toEqual(["neutral", "neutral"]);
    unmount();
    render(<Yesterday label="Tue 17 Nov" prs={{ closed: 5, merged: 5, opened: 2 }} reviews={2} />);
    expect(tone(within(screen.getByText("Yesterday").closest("summary")!).getByText("+3"))).toBe("ok");
    cleanup();
    render(<Yesterday label="Tue 17 Nov" prs={{ closed: 2, merged: 2, opened: 2 }} reviews={2} />);
    expect(tone(within(screen.getByText("Yesterday").closest("summary")!).getByText("0"))).toBe("neutral");
  });

  it("draws throughput per week as labelled images", async () => {
    render(
      <ThroughputMinis
        weeks={[
          { label: "2 Nov", merged: 9, reviews: 6, partial: false },
          { label: "9 Nov", merged: 14, reviews: 5, partial: false },
          { label: "16 Nov", merged: 10, reviews: 5, partial: true },
        ]}
      />,
    );
    expect(screen.getByText(/merged last week/)).toHaveTextContent("14 merged last week");
    await userEvent.click(screen.getByText("Throughput, 3 weeks"));
    expect(
      screen.getByRole("img", { name: "Your PRs merged: 2 Nov 9, 9 Nov 14, 16 Nov 10 (last so far)" }),
    ).toBeInTheDocument();
    expect(screen.getAllByRole("img")).toHaveLength(2);
    // The running week is dashed, not lighter (opacity darkens it on the dark theme).
    const partial = [...document.querySelectorAll("rect[data-partial]")];
    expect(partial).toHaveLength(2);
    for (const bar of partial) {
      expect(bar).toHaveAttribute("stroke-dasharray");
      expect(bar).not.toHaveAttribute("opacity");
    }
    expect(screen.getByText("The last bar is this week so far (dashed).")).toBeInTheDocument();
  });

  it("buckets review ages and draws them on one scale", async () => {
    const since = SUMMARY.snapshot!.awaiting.map((item) => item.waiting_since);
    expect(ageBuckets(since, NOW)).toEqual([1, 1, 1, 0]);
    render(<QueueAge yoursWaiting={ageBuckets(since, NOW)} waitingOnYou={null} />);
    await userEvent.click(screen.getByText("Review queue age"));
    expect(
      screen.getByRole("img", { name: "Your PRs waiting on others: ≤ 2 d 1, 3–7 d 1, 8–14 d 1, > 14 d 0" }),
    ).toBeInTheDocument();
    // The age of review requests to the user is not in the scan data: said, not silently dropped.
    expect(screen.getByText("Review-request age is not collected yet.")).toBeInTheDocument();
  });

  it("lists scan notes, repositories the token cannot see and stale TODOs", () => {
    render(
      <ScanNotes
        notes={SUMMARY.snapshot!.notes}
        unreachable={["acme/legacy-tools"]}
        todos={{ status: "not_refreshed", read_at: null, failed_at: "2026-11-18T10:20:00Z" }}
        now={NOW}
      />,
    );
    const items = screen.getAllByRole("listitem").map((li) => li.textContent);
    expect(items).toHaveLength(5);
    expect(items[0]).toBe(
      "Not visible to your GitHub token: acme/legacy-tools — install the GitHub App there or grant it access.",
    );
    expect(items[1]).toMatch(/^MemCan TODOs not refreshed \(MemCan unreachable at \d\d:\d\d\)/);
  });

  it("with notes, offers Open Settings (most notes are fixed there); without notes it does not", async () => {
    const onNavigate = vi.fn();
    const { unmount } = render(
      <ScanNotes
        notes={[{ text: "GitHub search refuses alice-bot.", item: null }]}
        unreachable={[]}
        todos={{ status: "read", read_at: null, failed_at: null }}
        settingsHref="/settings"
        onNavigate={onNavigate}
        now={NOW}
      />,
    );
    await userEvent.click(screen.getByText("What the scan missed"));
    const link = screen.getByRole("link", { name: "Open Settings" });
    expect(link).toHaveAttribute("href", "/settings");
    expect(link).toHaveAccessibleDescription(/Settings → Dashboard/);
    await userEvent.click(link);
    expect(onNavigate).toHaveBeenCalledWith("/settings");
    unmount();
    render(
      <ScanNotes
        notes={[]}
        unreachable={[]}
        todos={{ status: "read", read_at: null, failed_at: null }}
        settingsHref="/settings"
        now={NOW}
      />,
    );
    expect(screen.queryByRole("link", { name: "Open Settings" })).toBeNull();
  });
});
