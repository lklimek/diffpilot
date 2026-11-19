import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import type { FilesResponse, PrFile, Tour } from "../../api/generated/types";
import { expectTrustWording } from "../../__tests__/wording";
import seed from "../../mocks/files-482.json";
import { seedFor } from "../../mocks/fixtures";
import { FileHeader, type ViewedControl } from "./FileHeader";
import { FileTree } from "./FileTree";
import { FlowsAtLines, NoFlowPart } from "./FlowsAtLines";
import { flowsAt } from "./model";
import { ViewedChip } from "./ViewedChip";

const tour = seedFor("default", "tour") as Tour;
const files = (seed as FilesResponse).files;
const file = (path: string) => files.find((f) => f.path === path)!;
const client = file("src/rpc/client.rs");

function Tree({ onSelect = () => {}, status = "checked" as FilesResponse["viewed_status"], list = files }) {
  const [filter, setFilter] = useState("");
  return (
    <FileTree
      files={list}
      tour={tour}
      ticked={["f1.2", "f2.1"]}
      current="src/rpc/client.rs"
      viewedStatus={status}
      filter={filter}
      onFilter={setFilter}
      onSelect={onSelect}
    />
  );
}

describe("FileTree", () => {
  it("groups files (Code first, Not analysed by path rule), with Viewed, step ticks and Unexplained lines", async () => {
    const onSelect = vi.fn();
    render(<Tree onSelect={onSelect} />);
    expect(screen.getByText(/Viewed on GitHub:/)).toHaveTextContent("Viewed on GitHub: 3 / 13 files");
    const headings = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    expect(headings).toEqual([
      "Code · 6",
      expect.stringMatching(/^Tests · 4classified by path/),
      expect.stringMatching(/^Docs & comments · 2classified by path/),
      expect.stringMatching(/^Mechanical & generated · 1classified by path/),
    ]);
    const current = screen.getByRole("button", { name: /client\.rs/ });
    expect(current).toHaveAttribute("aria-current", "page");
    expect(current).toHaveTextContent("not viewed");
    expect(current).toHaveTextContent("flows 1, 4 · 1/9 steps ticked");
    const config = screen.getByRole("button", { name: /config\.rs/ });
    expect(config).toHaveTextContent("viewed on GitHub");
    expect(config).toHaveTextContent("1/1 steps ticked");
    expect(screen.getByRole("button", { name: /error\.rs/ })).toHaveTextContent("changed since viewed");
    expect(screen.getByRole("button", { name: /metrics\.rs/ })).toHaveTextContent("Unexplained 6");
    expect(screen.getByRole("button", { name: /send_once\.rs/ })).toHaveTextContent("deleted");
    expect(screen.getByRole("button", { name: /Cargo\.lock/ })).toHaveTextContent("not shown");
    expect(screen.getByRole("button", { name: /src\/rpc\/retry\.rs/ })).toHaveTextContent(
      "renamed from src/util/backoff.rs",
    );
    await userEvent.click(screen.getByRole("button", { name: /lib\.rs/ }));
    expect(onSelect).toHaveBeenCalledWith("src/lib.rs");
    await userEvent.type(screen.getByRole("searchbox", { name: "Filter files" }), "rpc/");
    expect(screen.queryByRole("button", { name: /config\.rs/ })).toBeNull();
    expect(screen.getByRole("button", { name: /client\.rs/ })).toBeVisible();
    expectTrustWording(document.body.textContent ?? "");
  });

  it("says plainly when GitHub's Viewed state was not read; nothing is assumed", () => {
    render(<Tree status="not_checked" list={files.map((f) => ({ ...f, viewed: null }))} />);
    expect(screen.getByText(/Viewed state not read from GitHub/)).toHaveTextContent("nothing is assumed");
    expect(screen.getByRole("button", { name: /client\.rs/ })).toHaveTextContent("Viewed not read");
  });
});

const pr = tour.analysis.pr;
const control = (over: Partial<ViewedControl> = {}): ViewedControl => ({
  state: "unviewed",
  saving: false,
  disabledReason: null,
  error: null,
  onToggle: vi.fn(),
  ...over,
});

describe("FileHeader", () => {
  it("names the file with its change, counts and GitHub link; Viewed is a toggle saved to GitHub", async () => {
    const viewed = control();
    const onToggle = vi.fn();
    render(<FileHeader file={client} pr={pr} viewed={viewed} oldCode={{ shown: false, onToggle }} />);
    expect(screen.getByRole("heading", { level: 2 })).toHaveTextContent("src/rpc/client.rs");
    expect(screen.getByText("modified")).toBeVisible();
    expect(screen.getByRole("banner")).toHaveTextContent("+62 −57");
    expect(screen.getByRole("link", { name: /GitHub/ })).toHaveAttribute(
      "href",
      expect.stringMatching(
        /^https:\/\/github\.com\/acme\/node-client\/pull\/482\/changes#diff-[0-9a-f]{64}$/,
      ),
    );
    const toggle = screen.getByRole("button", { name: "Viewed" });
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    expect(toggle).toHaveAccessibleDescription(/Your GitHub 'Viewed' mark .* \(v\)/);
    await userEvent.click(toggle);
    expect(viewed.onToggle).toHaveBeenCalledOnce();
    await userEvent.click(screen.getByRole("button", { name: "Show old code" }));
    expect(onToggle).toHaveBeenCalledOnce();
  });

  it("waits for GitHub, shows a refusal plainly with Retry, and stays disabled with a reason", async () => {
    const saving = control({ saving: true });
    const { rerender } = render(<FileHeader file={client} pr={pr} viewed={saving} />);
    // The saving state is the button's name, so a screen reader hears it.
    const toggle = screen.getByRole("button", { name: "Saving to GitHub…" });
    expect(toggle).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(toggle);
    expect(saving.onToggle).not.toHaveBeenCalled();
    const retry = vi.fn();
    rerender(
      <FileHeader
        file={client}
        pr={pr}
        viewed={control({ error: { message: "GitHub refused it.", retry } })}
      />,
    );
    const alert = screen.getByRole("alert");
    // The server's message is written for the reader: shown as is, once.
    expect(alert.querySelector("p")).toHaveTextContent(/^GitHub refused it\.$/);
    await userEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    expect(retry).toHaveBeenCalledOnce();
    expect(within(alert).getByRole("link", { name: /Mark it on GitHub/ })).toHaveAttribute(
      "href",
      expect.stringContaining("/pull/482/changes#diff-"),
    );
    const moved = control({ disabledReason: "This analysis is for 9f3c2e1; the PR is now at d41b7a0." });
    rerender(<FileHeader file={client} pr={pr} viewed={moved} />);
    await userEvent.click(screen.getByRole("button", { name: "Viewed" }));
    expect(moved.onToggle).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Viewed" })).toHaveAccessibleDescription(/the PR is now at/);
    rerender(<FileHeader file={client} pr={pr} viewed={control({ state: "dismissed" })} />);
    expect(screen.getByText(/Changed since you marked it viewed/)).toHaveTextContent(
      "GitHub cleared the mark when new commits changed this file",
    );
  });
});

describe("ViewedChip", () => {
  it.each([
    ["viewed", "☑ viewed"],
    ["dismissed", "↻ changed since viewed"],
    ["unviewed", "☐ not viewed"],
    [null, "? Viewed not read"],
  ] as const)("%s reads %s, with a tip about where it is set", (state, text) => {
    render(<ViewedChip state={state} path="src/a.rs" />);
    const chip = document.querySelector<HTMLElement>("[data-viewed]")!;
    expect(chip).toHaveTextContent(text);
    expect(chip).toHaveAccessibleDescription(/src\/a\.rs/);
  });
});

describe("FlowsAtLines", () => {
  const hunk = (f: PrFile, id: string) => f.hunks.find((h) => h.id === id)!;

  it("lists each flow with its steps (Explained in / Also shown in), tick state read-only, Details, Open in Flows", async () => {
    const onDetails = vi.fn();
    const onOpenInFlows = vi.fn();
    const flows = flowsAt(tour, [...hunk(client, "h17").parts, ...hunk(client, "h19").parts]);
    render(
      <FlowsAtLines
        flows={flows}
        ticked={["f1.3"]}
        analysis={tour.analysis.state}
        onDetails={onDetails}
        onOpenInFlows={onOpenInFlows}
      />,
    );
    const flow = screen.getByRole("group", { name: /^Flow 1/ });
    expect(flow).toHaveTextContent("Why this flow");
    const steps = within(flow).getAllByRole("listitem");
    expect(steps[0]).toHaveTextContent("Explained in");
    expect(steps[0]).toHaveTextContent("Step 2");
    expect(steps[0]).toHaveTextContent("☐ not ticked in Flows");
    expect(steps[1]).toHaveTextContent("☑ ticked in Flows");
    expect(screen.queryByRole("button", { name: /Mark reviewed/ })).toBeNull();
    await userEvent.click(within(steps[0]!).getByRole("button", { name: /^Details/ }));
    expect(onDetails).toHaveBeenCalledWith("f1.2");
    await userEvent.click(within(steps[0]!).getByRole("button", { name: /^Open in Flows/ }));
    expect(onOpenInFlows).toHaveBeenCalledWith("f1.2");
    expectTrustWording(document.body.textContent ?? "");
  });

  it("marks a step that only shows the lines as Also shown in", () => {
    render(
      <FlowsAtLines
        flows={flowsAt(tour, hunk(client, "h17").parts)}
        ticked={[]}
        analysis={tour.analysis.state}
        onDetails={() => {}}
        onOpenInFlows={() => {}}
      />,
    );
    expect(screen.getByRole("listitem")).toHaveTextContent("Also shown in");
  });

  it("a flow not analysed yet says so and offers to analyse it, which starts LLM work", async () => {
    const lazy = structuredClone(tour);
    lazy.flows[1]!.steps = [];
    lazy.flows[1]!.analysis_status = { state: "not_analysed", started_at: null, message: null };
    lazy.flows[1]!.planned = { intent: "Shorten the default timeout.", importance_rank: 2, ranges: [] };
    const onRequestFlow = vi.fn();
    render(
      <FlowsAtLines
        flows={flowsAt(lazy, [
          {
            side: "head",
            range: { start_line: 23, end_line: 23 },
            owner: { kind: "flow", flow_id: "f2" },
            also_in: [],
          },
        ])}
        ticked={[]}
        analysis={lazy.analysis.state}
        onDetails={() => {}}
        onOpenInFlows={() => {}}
        onRequestFlow={onRequestFlow}
      />,
    );
    const flow = screen.getByRole("group", { name: /^Flow 2/ });
    expect(flow).toHaveTextContent("not analysed yet");
    expect(flow).toHaveTextContent("Shorten the default timeout.");
    const analyse = within(flow).getByRole("button", { name: "▶ Analyse this flow" });
    expect(analyse).toHaveAccessibleDescription(/starts LLM work/);
    await userEvent.click(analyse);
    expect(onRequestFlow).toHaveBeenCalledWith("f2");
  });
});

describe("NoFlowPart", () => {
  it.each([
    [{ kind: "unexplained" } as const, "In no flow. Unexplained — read these yourself."],
    [
      { kind: "not_analysed", category: "tests", badge: "classified_by_path", rule: "tests/**" } as const,
      "In no flow. Not analysed: recognised by file path (rule tests/**)",
    ],
    [
      { kind: "context_rule", badge: "classified_by_line_rule", rule: "comment" } as const,
      "In no flow. Covered by a line rule (comment)",
    ],
    [
      {
        kind: "identical_move",
        from: { file: "a.rs", side: "base", range: { start_line: 1, end_line: 2 } },
        to: { file: "b.rs", side: "head", range: { start_line: 5, end_line: 6 } },
      } as const,
      "In no flow. Moved with identical text: from a.rs:1–2 to b.rs:5–6.",
    ],
    [{ kind: "whitespace_only" } as const, "In no flow. Only whitespace changed"],
  ])("states %o factually", (owner, text) => {
    render(
      <NoFlowPart part={{ side: "head", range: { start_line: 41, end_line: 46 }, owner, also_in: [] }} />,
    );
    expect(document.body).toHaveTextContent(text);
    expectTrustWording(document.body.textContent ?? "");
  });
});
