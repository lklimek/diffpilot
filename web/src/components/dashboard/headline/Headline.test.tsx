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
