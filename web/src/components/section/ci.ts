import type { CiCounts, CiStatus } from "../../api/generated/types";
function counts(...sources: (CiCounts | undefined)[]): string {
  if (sources.every((value) => value === undefined)) return "not checked";
  return (
    (["passed", "failed", "pending", "skipped", "neutral"] as const)
      .flatMap((key) => {
        const count = sources.reduce((sum, value) => sum + (value?.[key] ?? 0), 0);
        return count > 0 ? [`${count} ${key}`] : [];
      })
      .join(", ") || "none reported"
  );
}

/** The sum of the outcomes GitHub reported; the tooltip keeps the per-source evidence. */
export const ciBadgeText = (ci: CiStatus) => `CI: ${counts(ci.check_runs, ci.statuses)}`;

/** Exact GitHub source counts; absent source data never implies success. */
export function ciText(ci: CiStatus): string {
  const sources = `Check runs: ${counts(ci.check_runs)}. Statuses: ${counts(ci.statuses)}.`;
  const allPassing =
    ci.check_runs &&
    ci.statuses &&
    ci.check_runs.passed + ci.statuses.passed > 0 &&
    [ci.check_runs, ci.statuses].every((c) => c.failed + c.pending + c.skipped + c.neutral === 0);
  if (allPassing) return `${sources} CI: all passing.`;
  if (ci.state === "no_failures")
    return `${sources} Nothing failed among the checks read, but not everything ran successfully.`;
  return sources;
}
/** Tooltip of the CI summary when it links to the PR's checks on GitHub. */
export const CHECKS_LINK_TIP = "Open the checks on GitHub (new tab).";

/** Tooltip identifies what was actually read, including partial GitHub permission failures. */
export function ciTip(ci: CiStatus): string {
  return `${ciText(ci)} From GitHub for the head commit. ${ci.failing.length ? `Failing checks: ${ci.failing.map((c) => c.name).join(", ")}.` : ""}`.trim();
}
