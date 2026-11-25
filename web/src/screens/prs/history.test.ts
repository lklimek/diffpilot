import { expect, it } from "vitest";
import { historyFromLegacy } from "./history";
import type { MyAnalysis, WatchedPr } from "../../api/generated/types";

const pr = (number: number) => ({ repo: { owner: "acme", name: "repo" }, number });
const analysis = (number: number, created_at: string): MyAnalysis => ({
  id: `analysis-${number}`,
  pr: pr(number),
  title: `Analysis of #${number}`,
  status: "finished",
  created_at,
});
const watch = (number: number, ended_at: string | null): WatchedPr => ({
  pr: pr(number),
  title: `Watch of #${number}`,
  since: "2026-11-20T09:00:00Z",
  last_probe_at: null,
  last_auto_pass: null,
  state: ended_at ? "ended" : "active",
  ended_at,
  badge: { badge: null, facts: [], missing: [] },
});

it("orders the union newest first, using the later activity for a PR with both sources", () => {
  const ended = "2026-11-25T09:00:00Z";
  const rows = historyFromLegacy(
    [analysis(1, "2026-11-21T09:00:00Z"), analysis(2, "2026-11-24T09:00:00Z")],
    [watch(3, null), watch(1, ended), watch(2, null)],
  );
  expect(rows.map(({ pr: { number }, last_activity_at }) => [number, last_activity_at])).toEqual([
    [1, ended],
    [2, "2026-11-24T09:00:00Z"],
    [3, "2026-11-20T09:00:00Z"],
  ]);
  expect(rows[0]).toMatchObject({ analysis: { id: "analysis-1" }, watch: { state: "ended" } });
  expect(rows[1]).toMatchObject({ title: "Analysis of #2", watch: { state: "active" } });
});
