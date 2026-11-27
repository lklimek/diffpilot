import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { HttpResponse, http } from "msw";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it } from "vitest";
import { startMockServer } from "../../__tests__/screen";
import type { LinesResponse, RangeQuery } from "../../api/generated/types";
import { ChunkedCode, FileDiffCode, MAX_RANGE_LINES, RangeCode, rangeChunks, type CodeContext } from "./code";

const server = startMockServer();
let requests: RangeQuery[] = [];
const DELETED = 5001;

beforeEach(() => {
  requests = [];
  server.use(
    http.get("/api/v1/analyses/:id/lines", ({ request }) => {
      const q = new URL(request.url).searchParams;
      const query: RangeQuery = {
        path: q.get("path")!,
        side: q.get("side") as RangeQuery["side"],
        from: Number(q.get("from")),
        to: Number(q.get("to")),
      };
      requests.push(query);
      if (query.to - query.from + 1 > MAX_RANGE_LINES) {
        return HttpResponse.json({ code: "range_too_large", message: "Too many lines." }, { status: 400 });
      }
      const lines: LinesResponse = {
        path: query.path,
        side: query.side,
        from: query.from,
        to: query.to,
        total_lines: 10_000,
        lines: Array.from({ length: query.to - query.from + 1 }, (_, i) => `line ${query.from + i}`),
        changed: [],
        // The head range 1..1 carries a removed block of 5001 lines, 2000 of them in this response.
        deleted:
          query.side === "head" && query.from === 1
            ? [{ after_line: 1, base_start: 100, total_lines: DELETED, lines: Array(2000).fill("gone") }]
            : [],
        moved: [],
        comments: { scope: "in_diff", line_thread_ranges: [] },
      };
      return HttpResponse.json(lines);
    }),
  );
});

const ctx = (activeLine: CodeContext["activeLine"] = null): CodeContext => ({
  analysisId: "6d3a2f1e-0482-4c5e-9a7b-000000000482",
  threads: [],
  comments: [],
  revealComment: null,
  onEditComment: () => Promise.resolve(),
  onDeleteComment: () => Promise.resolve(),
  activeLine,
  onComment: () => {},
});

function show(node: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
}

describe("code of long and disjoint ranges", () => {
  it("makes the whole test file's classic diff accessible beyond the range limit", async () => {
    const seen: number[] = [];
    server.use(
      http.get("/api/v1/analyses/:id/diff", ({ request }) => {
        const q = new URL(request.url).searchParams;
        seen.push(Number(q.get("from")));
        return HttpResponse.json({ path: "tests/large.rs", side: "head", hunks: [] });
      }),
    );
    show(<FileDiffCode ctx={ctx()} path="tests/large.rs" side="head" />);
    await screen.findByRole("button", { name: "Show diff lines 2001–4000" });
    await userEvent.click(screen.getByRole("button", { name: "Show diff lines 2001–4000" }));
    await screen.findByRole("button", { name: "Show diff lines 4001–6000" });
    expect(seen).toEqual([1, 2001]);
    expect(requests.map((q) => [q.from, q.to])).toEqual([[1, 1]]);
  });

  it("splits ranges into requests of at most the server limit, never joining disjoint ranges", () => {
    expect(
      rangeChunks("a.rs", "head", [
        { start_line: 10, end_line: 10 },
        { start_line: 3010, end_line: 7100 },
      ]).map((q) => [q.from, q.to]),
    ).toEqual([
      [10, 10],
      [3010, 5009],
      [5010, 7009],
      [7010, 7100],
    ]);
  });

  it("shows disjoint Unexplained ranges one by one, without the lines between them", async () => {
    show(
      <ChunkedCode
        ctx={ctx()}
        path="a.rs"
        side="head"
        ranges={[
          { start_line: 10, end_line: 10 },
          { start_line: 3010, end_line: 3010 },
        ]}
      />,
    );
    expect(await screen.findByRole("region", { name: "a.rs, lines 10 to 10" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Show lines 3010–3010 of a.rs" }));
    expect(await screen.findByRole("region", { name: "a.rs, lines 3010 to 3010" })).toBeInTheDocument();
    expect(requests.map((q) => [q.from, q.to])).toEqual([
      [10, 10],
      [3010, 3010],
    ]);
  });

  it("reads the rest of a long removed block in parts of at most the limit", async () => {
    show(<RangeCode ctx={ctx()} query={{ path: "a.rs", side: "head", from: 1, to: 1 }} />);
    const user = userEvent.setup();
    const moreDeleted = await screen.findByLabelText("Show 3001 more deleted lines", { selector: "button" });
    expect(moreDeleted).toHaveAccessibleName("Show 3001 more deleted lines");
    await user.click(moreDeleted);
    await screen.findByLabelText("a.rs, lines 2100 to 4099, removed code at the base commit", {
      selector: '[role="region"]',
    });
    const next = screen.getByText("Show lines 4100–5100 of a.rs (base commit)", { selector: "button" });
    expect(next).toHaveAccessibleName("Show lines 4100–5100 of a.rs (base commit)");
    await user.click(next);
    await screen.findByLabelText("a.rs, lines 4100 to 5100, removed code at the base commit", {
      selector: '[role="region"]',
    });
    expect(requests.slice(1).map((q) => [q.side, q.from, q.to])).toEqual([
      ["base", 2100, 4099],
      ["base", 4100, 5100],
    ]);
    expect(screen.queryByText(/Too many lines/)).toBeNull();
  });
});

describe("the line being commented on", () => {
  it("is marked only in the view of its own file", async () => {
    const target = { path: "a.rs", side: "head" as const, start_line: null, line: 10 };
    show(
      <>
        <RangeCode ctx={ctx(target)} query={{ path: "a.rs", side: "head", from: 10, to: 11 }} />
        <RangeCode ctx={ctx(target)} query={{ path: "b.rs", side: "head", from: 10, to: 11 }} />
      </>,
    );
    const a = await screen.findByRole("region", { name: "a.rs, lines 10 to 11" });
    const b = await screen.findByRole("region", { name: "b.rs, lines 10 to 11" });
    expect(within(a).getByText(/commenting on this line/)).toBeInTheDocument();
    expect(within(b).queryByText(/commenting on this line/)).toBeNull();
  });
});
