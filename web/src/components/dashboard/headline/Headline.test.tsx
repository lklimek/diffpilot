import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it } from "vitest";
import { netRows, type NetPoint } from "../net-chart/NetChart";
import { Headline, type ChartUnit, type HeadlineFigures } from "./Headline";

const point = (label: string, closed: number, opened: number, partial = false): NetPoint => ({
  label,
  full: `Day ${label}`,
  closed,
  opened,
  partial,
  tick: true,
});
const DAYS = [point("16 Nov", 3, 1), point("17 Nov", 6, 2), point("18 Nov", 2, 2, true)];
const WEEKS = [point("2 Nov", 9, 7), point("9 Nov", 11, 5), point("16 Nov", 11, 5, true)];
const FIGURES: HeadlineFigures = {
  week: { closed: 11, opened: 5, label: "Mon–Wed" },
  yesterday: { closed: 6, opened: 2, label: "Tue 17 Nov" },
  lastWeek: { closed: 11, opened: 5, label: "9–13 Nov" },
  fourWeeks: { net: 19, before: -14 },
};

function Harness() {
  const [unit, setUnit] = useState<ChartUnit>("day");
  return <Headline figures={FIGURES} days={DAYS} weeks={WEEKS} unit={unit} onUnitChange={setUnit} />;
}

describe("netRows", () => {
  it("nets closed minus opened and keeps a running total", () => {
    expect(netRows(DAYS).map((r) => [r.net, r.running])).toEqual([
      [2, 2],
      [4, 6],
      [0, 6],
    ]);
  });
});

describe("Headline", () => {
  it("shows PRs closed minus opened this week, and the tiles", () => {
    render(<Harness />);
    expect(document.querySelector("[data-headline-value]")).toHaveTextContent("+6");
    expect(screen.getByText(/Mon–Wed: closed/)).toHaveTextContent("Mon–Wed: closed 11 · opened 5 PRs");
    expect(screen.getByText("Yesterday").parentElement).toHaveTextContent("+4");
    expect(screen.getByText("4 weeks").parentElement).toHaveTextContent("+19before: −14");
    // OD16: no PRs / issues switch on the statistics.
    expect(screen.queryByRole("button", { name: /issues/i })).toBeNull();
  });

  it("colours every net by its sign (closed more: ok, opened more: high, zero: neutral); counts stay neutral", async () => {
    const figures: HeadlineFigures = {
      week: { closed: 2, opened: 5, label: "Mon–Wed" },
      yesterday: { closed: 3, opened: 3, label: "Tue 17 Nov" },
      lastWeek: { closed: 7, opened: 4, label: "9–13 Nov" },
      fourWeeks: { net: -2, before: 5 },
    };
    const days = [point("16 Nov", 1, 3), point("17 Nov", 4, 1), point("18 Nov", 2, 2, true)];
    render(<Headline figures={figures} days={days} weeks={WEEKS} unit="day" onUnitChange={() => {}} />);
    const tone = (el: Element | null) =>
      el?.classList.contains("text-ok") ? "ok" : el?.classList.contains("text-high") ? "high" : "neutral";
    const headline = document.querySelector("[data-headline-value]");
    expect([headline?.textContent, tone(headline)]).toEqual(["−3", "high"]);
    const tile = (label: string) => screen.getByText(label).parentElement!;
    expect(tone(within(tile("Yesterday")).getByText("0"))).toBe("neutral");
    expect(tone(within(tile("Last week")).getByText("+3"))).toBe("ok");
    expect(tone(within(tile("4 weeks")).getByText("−2"))).toBe("high");
    expect(tone(within(tile("4 weeks")).getByText("+5"))).toBe("ok");
    expect(tone(within(tile("Yesterday")).getByText("3 − 3"))).toBe("neutral");
    await userEvent.click(screen.getByRole("button", { name: "Show as table" }));
    // Newest first: today (0, running +1), 17 Nov (+3, +1), 16 Nov (−2, −2).
    const cells = within(screen.getByRole("table"))
      .getAllByRole("row")
      .slice(1)
      .map((row) => within(row).getAllByRole("cell").slice(1).map(tone));
    expect(cells).toEqual([
      ["neutral", "neutral", "neutral", "ok"],
      ["neutral", "neutral", "ok", "ok"],
      ["neutral", "neutral", "high", "high"],
    ]);
  });

  it("switches between days and weeks with pressed buttons", async () => {
    render(<Harness />);
    expect(screen.getByRole("button", { name: "Days" })).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(screen.getByRole("button", { name: "Weeks" }));
    expect(screen.getByRole("button", { name: "Weeks" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("heading", { name: "Net per week, with cumulative total" })).toBeInTheDocument();
    expect(screen.getByRole("img")).toHaveAccessibleName(
      /per week for the last 3 weeks; cumulative total \+14/,
    );
  });
});

describe("NetChart keyboard reading", () => {
  it("reads the last point on focus and moves with ←, →, Home and End into a live region", () => {
    render(<Harness />);
    const chart = screen.getByRole("img");
    const reading = () => document.querySelector("[data-chart-reading]")!.textContent;
    fireEvent.focus(chart);
    expect(reading()).toBe("Day 18 Nov (so far): closed 2, opened 2, net 0, cumulative +6");
    fireEvent.keyDown(chart, { key: "ArrowLeft" });
    expect(reading()).toBe("Day 17 Nov: closed 6, opened 2, net +4, cumulative +6");
    fireEvent.keyDown(chart, { key: "Home" });
    expect(reading()).toMatch(/^Day 16 Nov/);
    fireEvent.keyDown(chart, { key: "ArrowLeft" });
    expect(reading()).toMatch(/^Day 16 Nov/);
    fireEvent.keyDown(chart, { key: "End" });
    expect(reading()).toMatch(/^Day 18 Nov/);
    expect(chart).toHaveAttribute("tabindex", "0");
  });

  it("offers the same numbers as a table", async () => {
    render(<Harness />);
    const toggle = screen.getByRole("button", { name: "Show as table" });
    await userEvent.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    const rows = within(screen.getByRole("table")).getAllByRole("row");
    expect(rows[1]).toHaveTextContent("Day 18 Nov (so far)220+6");
    expect(rows[0]).toHaveTextContent("DayClosedOpenedNetCumulative");
    // A long table scrolls: its scroll box takes keyboard focus and has a name.
    const box = screen.getByRole("region", { name: "Net per day as a table" });
    expect(box).toHaveAttribute("tabindex", "0");
    expect(box).toContainElement(screen.getByRole("table"));
  });

  it("marks today so far with a dashed outline, also when its net is 0", () => {
    render(<Harness />);
    const today = document.querySelector('[data-point="2"] [data-partial]')!;
    expect(today).toHaveAttribute("stroke-dasharray");
    expect(today).not.toHaveAttribute("opacity");
    expect(document.querySelectorAll("[data-partial]")).toHaveLength(1);
  });

  it("draws bars and the line with theme tokens only", () => {
    render(<Harness />);
    const svg = screen.getByRole("img");
    expect(svg.querySelectorAll("path.fill-c-pos").length).toBe(2);
    expect(svg.innerHTML).not.toMatch(/#[0-9a-f]{3,6}\b|rgb\(/i);
  });
});

describe("NetChart: a bar lists its pull requests", () => {
  const LISTED = {
    opened: [{ id: "acme/a#5", title: "feat: five", url: "https://github.com/acme/a/pull/5" }],
    closed: [],
  };

  function Selecting() {
    const [unit, setUnit] = useState<ChartUnit>("day");
    const [selected, setSelected] = useState<number | null>(null);
    return (
      <Headline
        figures={FIGURES}
        days={DAYS}
        weeks={WEEKS}
        unit={unit}
        onUnitChange={setUnit}
        selected={selected}
        onSelect={setSelected}
        listed={selected === null ? null : LISTED}
      />
    );
  }

  // Without layout the chart is 720 px wide: three slots of (720 − 34 − 46) / 3 px after the left margin.
  const clickBar = (i: number) =>
    fireEvent.click(screen.getByRole("img"), { clientX: 34 + ((720 - 80) / 3) * (i + 0.5) });
  const list = (name: string) => screen.queryByRole("region", { name: `Pull requests of ${name}` });

  it("a click on a bar lists its pull requests under the chart; a second click closes the list", () => {
    render(<Selecting />);
    clickBar(1);
    expect(list("Day 17 Nov")).toBeVisible();
    // The tip follows the clicked bar, also when the click also focused the chart.
    expect(document.querySelector("[data-chart-tip]")).toHaveTextContent(
      /^Day 17 Nov.*Click or press Enter to hide its pull requests$/,
    );
    expect(document.querySelector('[data-point="1"][data-selected]')).not.toBeNull();
    expect(screen.getByRole("link", { name: /^acme\/a#5/ })).toBeVisible();
    clickBar(2);
    expect(list("Day 17 Nov")).toBeNull();
    expect(list("Day 18 Nov (so far)")).toBeVisible();
    clickBar(2);
    expect(list("Day 18 Nov (so far)")).toBeNull();
    expect(document.querySelector("[data-selected]")).toBeNull();
  });

  it("Enter or Space lists the bar being read; again closes it; Hide returns the focus to the chart", async () => {
    render(<Selecting />);
    const chart = screen.getByRole("img");
    expect(chart).toHaveAccessibleName(/Enter or Space lists the pull requests of the day read/);
    chart.focus();
    fireEvent.keyDown(chart, { key: "ArrowLeft" });
    fireEvent.keyDown(chart, { key: "Enter" });
    expect(list("Day 17 Nov")).toBeVisible();
    expect(document.querySelector("[data-chart-reading]")).toHaveTextContent(
      "Pull requests of Day 17 Nov listed under the chart",
    );
    fireEvent.keyDown(chart, { key: " " });
    expect(list("Day 17 Nov")).toBeNull();
    fireEvent.keyDown(chart, { key: "Enter" });
    screen.getByRole("button", { name: "Hide pull requests of Day 17 Nov" }).focus();
    await userEvent.keyboard("{Enter}");
    expect(list("Day 17 Nov")).toBeNull();
    expect(chart).toHaveFocus();
    // Back on the chart, the hiding is announced, and Enter lists the bar just hidden again.
    expect(document.querySelector("[data-chart-reading]")).toHaveTextContent(
      "Pull requests of Day 17 Nov hidden",
    );
    fireEvent.keyDown(chart, { key: "Enter" });
    expect(list("Day 17 Nov")).toBeVisible();
  });

  it("Enter or Space after a click, with the pointer gone, acts on the clicked bar, not the last one", () => {
    render(<Selecting />);
    const chart = screen.getByRole("img");
    // A real click focuses the SVG (tabIndex 0) before the click event.
    fireEvent.mouseMove(chart, { clientX: 34 + ((720 - 80) / 3) * 1.5 });
    chart.focus();
    clickBar(1);
    expect(list("Day 17 Nov")).toBeVisible();
    fireEvent.mouseLeave(chart);
    fireEvent.keyDown(chart, { key: " " });
    expect(list("Day 18 Nov (so far)")).toBeNull();
    expect(list("Day 17 Nov")).toBeNull();
    expect(document.querySelector("[data-chart-reading]")).toHaveTextContent(
      "Pull requests of Day 17 Nov hidden",
    );
  });

  it("the hover tip says a bar can be clicked", () => {
    render(<Selecting />);
    fireEvent.mouseMove(screen.getByRole("img"), { clientX: 34 + (720 - 80) / 6 });
    expect(document.querySelector("[data-chart-tip]")).toHaveTextContent(
      "Click or press Enter to list its pull requests",
    );
  });

  it("without a selection handler the chart offers no listing", () => {
    render(<Harness />);
    const chart = screen.getByRole("img");
    expect(chart).not.toHaveAccessibleName(/Enter or Space/);
    clickBar(1);
    expect(document.querySelector("[data-selected]")).toBeNull();
    fireEvent.mouseMove(chart, { clientX: 34 + (720 - 80) / 6 });
    expect(document.querySelector("[data-chart-tip]")).not.toHaveTextContent(/Click/);
  });
});
